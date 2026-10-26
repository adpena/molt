from __future__ import annotations

from dataclasses import fields
import hashlib
import io
import json
import struct
from typing import Any

import msgspec
import pytest

from molt.cli import function_records as records
from molt.cli.cache_keys import FunctionDigests


def _function() -> dict[str, Any]:
    return {
        "name": "molt_init_fixture",
        "params": [],
        "return_abi": "void",
        "ops": [{"kind": "ret_void"}],
    }


def _wire(metadata: bytes, body: bytes, digest: bytes) -> bytes:
    # Independent persisted wire fixture, not the production record constructor.
    return msgspec.msgpack.encode(
        ["molt.lowering-record.v1", metadata, [[body, digest]]]
    )


def _digest(body: bytes) -> bytes:
    return hashlib.sha256(b"molt.function-body.msgpack.v1\0" + body).digest()


def test_backend_transports_have_canonical_bytes_after_record_hydration() -> None:
    import msgpack

    from molt.cli import cache_keys
    from molt.cli.backend_ir_transport import BackendIrProgram

    cold = _function()
    encoded, _seeds = records.encode_lowering_record(b"header", [cold])
    hydrated, _seeds = records.decode_function_records(
        records.decode_lowering_record(encoded)
    )
    expected_text = (
        '{"functions":[{"name":"molt_init_fixture","ops":[{"kind":"ret_void"}],'
        '"params":[],"return_abi":"void"}],"profile":"dev"}'
    )
    # A separately implemented codec receives an independently ordered literal:
    # the lease ships the exact body and names it by its digest.
    expected_body = msgpack.packb(json.loads(expected_text)["functions"][0])
    expected_binary = msgpack.packb(
        {
            "function_bodies": [expected_body],
            "function_digests": _digest(expected_body),
            "profile": "dev",
        },
        use_bin_type=True,
    )
    for function in (cold, hydrated[0], dict(reversed(cold.items()))):
        for ir in (
            {"profile": "dev", "functions": [function]},
            {"functions": [function], "profile": "dev"},
        ):
            assert cache_keys._backend_ir_text(ir) == expected_text
            streamed = io.StringIO()
            cache_keys._write_backend_ir_text(streamed, ir)
            assert streamed.getvalue() == expected_text
            binary = io.BytesIO()
            BackendIrProgram(ir, cache_keys.FunctionDigests()).write(binary)
            assert binary.getvalue() == expected_binary


def test_body_digest_domain_matches_the_backend_receiver() -> None:
    # runtime/molt-ir/src/addressed_program_tests.rs pins the same literals,
    # each computed by CPython's hashlib over the domain tag and the body.
    assert records.function_body_digest(b"").hex() == (
        "46cfe5b141fcf8a66c1b639d1f1efba63937ab8eee486e4ea0e917a05d1d0703"
    )
    assert records.function_body_digest(b"\x80").hex() == (
        "1b33b063cec4aa779ba4e30be9dec72b994a45727639f5f1bb35caa322a19f96"
    )


def test_record_roundtrip_preserves_rich_opaque_metadata_and_exact_body() -> None:
    function = _function()
    metadata = b'{"__integer_bytes__":[255,0],"text":"\\ud800"}\n'
    encoded, seeds = records.encode_lowering_record(metadata, [function])
    assert seeds[0].function is function
    raw = msgspec.msgpack.decode(encoded)
    assert raw[0] == "molt.lowering-record.v1"
    assert raw[1] == metadata
    body, digest = raw[2][0]
    assert digest == _digest(body) == seeds[0].digest
    assert msgspec.msgpack.decode(body) == function
    view = records.decode_lowering_record(encoded)
    assert view.metadata == metadata
    decoded, decoded_seeds = records.decode_function_records(view)
    assert decoded == [function]
    assert decoded[0] is not function
    assert decoded[0]["ops"] is not function["ops"]
    assert decoded_seeds[0].function is decoded[0]
    assert decoded_seeds[0].digest == digest
    assert [field.name for field in fields(records.FunctionDigestSeed)] == [
        "function",
        "digest",
    ]


def test_header_admission_does_not_materialize_or_validate_body(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class UnexpectedDecoder:
        def decode(self, _value: object) -> object:
            pytest.fail("body materialized before source/context admission")

    monkeypatch.setattr(records, "_BODY_DECODER", UnexpectedDecoder())
    # Even malformed bytes are opaque until the admitted header's next phase.
    view = records.decode_lowering_record(_wire(b"source-header", b"\xc1", b"bad"))
    assert view.metadata == b"source-header"
    assert len(view.functions) == 1
    assert isinstance(view.functions[0], msgspec.Raw)


def test_body_is_decoded_once_after_exact_digest_verification(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    original = records._BODY_DECODER
    calls: list[bytes] = []

    class ObservedDecoder:
        def decode(self, body: bytes) -> dict[str, Any]:
            calls.append(body)
            return original.decode(body)

    monkeypatch.setattr(records, "_BODY_DECODER", ObservedDecoder())
    body = msgspec.msgpack.encode(_function())
    with pytest.raises(ValueError, match="digest"):
        records.decode_function_records(
            records.decode_lowering_record(_wire(b"", body, b"x" * 32))
        )
    assert calls == []
    functions, seeds = records.decode_function_records(
        records.decode_lowering_record(_wire(b"", body, _digest(body)))
    )
    assert calls == [body]
    assert seeds[0].function is functions[0]


@pytest.mark.parametrize(
    "defect", ["version", "missing", "extra", "metadata", "trailing"]
)
def test_frame_shape_and_version_refused(defect: str) -> None:
    raw: list[Any] = ["molt.lowering-record.v1", b"", []]
    if defect == "version":
        raw[0] = "molt.lowering-record.v0"
    elif defect == "missing":
        raw.pop()
    elif defect == "extra":
        raw.append(None)
    elif defect == "metadata":
        raw[1] = "not bytes"
    encoded = msgspec.msgpack.encode(raw)
    if defect == "trailing":
        encoded += b"\xc0"
    with pytest.raises(ValueError):
        records.decode_lowering_record(encoded)


@pytest.mark.parametrize(
    "defect",
    ["hash", "short-hash", "invalid", "trailing", "list", "key", "record-extra"],
)
def test_function_record_corruption_refused(defect: str) -> None:
    body = msgspec.msgpack.encode(_function())
    if defect == "invalid":
        body = b"\xc1"
    elif defect == "trailing":
        body += b"\xc0"
    elif defect == "list":
        body = msgspec.msgpack.encode([])
    elif defect == "key":
        body = msgspec.msgpack.encode({1: "not a string key"})
    digest = _digest(body)
    if defect == "hash":
        digest = bytes([digest[0] ^ 1]) + digest[1:]
    elif defect == "short-hash":
        digest = digest[:-1]
    encoded = _wire(b"", body, digest)
    if defect == "record-extra":
        encoded = msgspec.msgpack.encode(
            ["molt.lowering-record.v1", b"", [[body, digest, None]]]
        )
    view = records.decode_lowering_record(encoded)
    with pytest.raises(ValueError):
        records.decode_function_records(view)


def test_pure_literals_and_key_order_have_one_exact_encoding() -> None:
    first: dict[str, Any] = {
        "name": "literals",
        "params": [],
        "ops": [
            {"kind": "const_float", "f_value": -0.0},
            {"kind": "const_float", "f_value": 0.0},
            {"kind": "const_float", "f_value": "NaN"},
            {"kind": "const_float", "f_value": "Infinity"},
            {"kind": "const_float", "f_value": "-Infinity"},
            {"kind": "const_bigint", "value": [0, 255, 128]},
            {"kind": "const_str", "value": [237, 160, 128]},
            {"kind": "const", "value": 1},
            {"kind": "const_float", "f_value": 1.0},
        ],
    }
    second = {
        "ops": [dict(reversed(list(op.items()))) for op in first["ops"]],
        "params": [],
        "name": "literals",
    }
    encoded = records.encode_function_body(first)
    assert encoded == records.encode_function_body(second)
    decoded = msgspec.msgpack.decode(encoded)
    assert json.dumps(decoded, sort_keys=True) == json.dumps(first, sort_keys=True)
    assert struct.pack("!d", decoded["ops"][0]["f_value"]) == struct.pack("!d", -0.0)
    assert type(decoded["ops"][7]["value"]) is int
    assert type(decoded["ops"][8]["f_value"]) is float
    assert records.function_body_digest(encoded) == _digest(encoded)
    assert records.function_body_digest(encoded) != hashlib.sha256(encoded).digest()


def test_seed_hit_and_copy_on_write_share_one_digest_owner(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_VERIFY_FUNCTION_DIGESTS", raising=False)
    original = _function()
    _, seeds = records.encode_lowering_record(b"", [original])
    changed = {**original, "ops": [{"kind": "const", "value": 1}]}
    equal_but_distinct = dict(original)
    calls: list[dict[str, Any]] = []
    encode = records.encode_function_body

    def observed(function: dict[str, Any]) -> bytes:
        calls.append(function)
        return encode(function)

    monkeypatch.setattr(records, "encode_function_body", observed)
    owner = FunctionDigests(seeds)
    assert owner.digest(original) == seeds[0].digest
    assert owner.digest(original) == seeds[0].digest
    assert calls == []
    assert owner.digest(changed) != seeds[0].digest
    assert owner.digest(changed) == owner.digest(changed)
    assert owner.digest(equal_but_distinct) == seeds[0].digest
    assert len(calls) == 2
    assert calls[0] is changed and calls[1] is equal_but_distinct


@pytest.mark.parametrize("verify", [False, True])
def test_all_seed_shapes_are_checked_before_owner_publication(
    monkeypatch: pytest.MonkeyPatch,
    verify: bool,
) -> None:
    monkeypatch.setenv("MOLT_VERIFY_FUNCTION_DIGESTS", "1" if verify else "0")
    function = _function()
    good = records.FunctionDigestSeed(function, b"x" * 32)
    bad = records.FunctionDigestSeed(function, b"short")
    owner = FunctionDigests.__new__(FunctionDigests)

    def unexpected(_function: object) -> bytes:
        pytest.fail("debug encoding started before all seed shapes were admitted")

    monkeypatch.setattr(records, "encode_function_body", unexpected)
    with pytest.raises(ValueError, match="invalid function digest seed"):
        owner.__init__([good, bad])
    assert not hasattr(owner, "_by_identity")


def test_conflicting_seed_identity_refused() -> None:
    function = _function()
    with pytest.raises(ValueError, match="conflicting"):
        FunctionDigests(
            [
                records.FunctionDigestSeed(function, b"x" * 32),
                records.FunctionDigestSeed(function, b"y" * 32),
            ]
        )


def test_debug_seed_verification_reencodes_once_and_refuses_stale_body(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_VERIFY_FUNCTION_DIGESTS", "1")
    function = _function()
    _, seeds = records.encode_lowering_record(b"", [function])
    encode = records.encode_function_body
    calls: list[dict[str, Any]] = []

    def observed(value: dict[str, Any]) -> bytes:
        calls.append(value)
        return encode(value)

    monkeypatch.setattr(records, "encode_function_body", observed)
    owner = FunctionDigests(seeds)
    assert len(calls) == 1 and calls[0] is function
    owner.digest(function)
    assert len(calls) == 1
    function["ops"] = [{"kind": "const", "value": 7}]
    with pytest.raises(ValueError, match="verification failed"):
        FunctionDigests(seeds)
