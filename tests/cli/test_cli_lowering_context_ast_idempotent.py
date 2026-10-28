"""The frontend-lowering context digest must be invariant to cache provenance.

A per-module lowering context payload can hold AST-bearing values (e.g. a class's
``inline_init_assigns``) in one of two provably-equivalent representations:

* a live ``ast.AST`` node, when the owning module was analysed fresh this build;
* a bare ``ast.dump`` string, when the class metadata was hydrated from the
  persisted analysis/lowering cache (the JSON decoder
  :func:`_decode_cached_json_value` collapses a stored ``{"__ast__": s}`` back to
  the bare string ``s``).

Both forms describe the *same* source and lower to byte-identical TIR, so the
context digest -- the shared per-module lowering cache's slot key -- MUST hash
them identically. Historically the encoder wrapped a live node in
``{"__ast__": s}`` while the decoder produced the bare string ``s``, so the
encode/decode round-trip was NOT idempotent: a fresh witness session computed a
different digest than the session that populated the shared cache, and the
cross-session lowering cache almost never hit (observed hit_rate ~0.19 on the
numpy witness; every class-bearing module missed).

These tests pin the fix as a two-direction miscompile-safety gate (the CODEX-B /
task #39 pattern):

* **Direction 1 (representation-only difference -> HIT):** a payload holding a
  live AST and the same payload after a cache round-trip (AST -> bare string)
  produce the *same* context digest. Without the fix these differ.
* **Direction 2 (real semantic change -> MISS):** changing the AST to a
  genuinely different node changes the digest, so a real class-metadata change
  still invalidates the persisted lowering rather than silently reusing a stale
  one.
"""

from __future__ import annotations

from molt.compiler_analysis.native_support_slice import ModuleBodySelection

import ast
import hashlib
import importlib
import json
from pathlib import Path
import sys

from molt.cli.module_source import _ModuleSourceLease

from molt.frontend import ClassId

CK = importlib.import_module("molt.cli.cache_keys")
MC = importlib.import_module("molt.cli.module_cache")


def _round_trip(payload: dict[str, object]) -> object:
    """Encode ``payload`` the way the caches do, then decode it back.

    Mirrors the real persistence path: ``json.dumps(..., default=_json_ir_default)``
    on write and :func:`_decode_cached_json_value` on read. The decoded object is
    what a warm cross-session build holds in memory for a cache-hydrated module.
    """
    encoded = json.dumps(
        CK._cache_json_value(payload), sort_keys=True, default=CK._json_ir_default
    )
    return MC._decode_cached_json_value(json.loads(encoded))


def _digest(payload: dict[str, object]) -> str | None:
    return MC._module_lowering_context_digest(payload)


def _payload_with_ast(node: ast.AST) -> dict[str, object]:
    # A minimal stand-in for the class-metadata shape that carries AST values on
    # the lowering context path (``known_classes`` -> class -> inline_init_assigns).
    return {
        "module_name": "pkg.mod",
        "known_classes": {
            ClassId("pkg.mod", "C"): {
                "methods": {
                    "__init__": {
                        "inline_init_assigns": [["attr", node]],
                    }
                }
            }
        },
    }


def test_json_ir_default_ast_encoding_is_idempotent() -> None:
    """The AST encode/decode round-trip is a fixed point at the encoder level.

    ``encode(node) == encode(decode(encode(node)))`` -- the invariant that keeps a
    fresh-analysis payload and a cache-hydrated payload byte-identical.
    """
    node = ast.parse("optionalRelease", mode="eval").body
    enc = json.dumps({"v": node}, sort_keys=True, default=CK._json_ir_default)
    dec = MC._decode_cached_json_value(json.loads(enc))
    enc2 = json.dumps(dec, sort_keys=True, default=CK._json_ir_default)
    assert enc == enc2, (
        "AST encode/decode is not idempotent -> a cache-hydrated lowering context "
        "hashes differently than a fresh one and the shared lowering cache misses"
    )
    # The canonical form is the bare ``ast.dump`` string (what the decoder yields),
    # never a ``{"__ast__": ...}`` wrapper.
    assert "__ast__" not in enc


def test_direction1_cache_provenance_does_not_change_context_digest() -> None:
    """Direction 1: fresh-AST payload and cache-hydrated payload -> SAME digest."""
    node = ast.parse("optionalRelease", mode="eval").body
    fresh = _payload_with_ast(node)
    hydrated = _round_trip(_payload_with_ast(node))

    fresh_digest = _digest(fresh)
    hydrated_digest = _digest(hydrated)

    assert fresh_digest is not None
    assert fresh_digest == hydrated_digest, (
        "context digest depends on cache provenance (fresh AST vs hydrated bare "
        "string) -> the cross-session shared lowering cache cannot hit"
    )


def test_direction2_real_ast_change_still_changes_context_digest() -> None:
    """Direction 2: a genuinely different AST value -> DIFFERENT digest (no stale reuse)."""
    node_a = ast.parse("optionalRelease", mode="eval").body
    node_b = ast.parse("mandatoryRelease", mode="eval").body

    digest_a = _digest(_payload_with_ast(node_a))
    digest_b = _digest(_payload_with_ast(node_b))

    assert digest_a is not None and digest_b is not None
    assert digest_a != digest_b, (
        "distinct class AST metadata collapsed to the same context digest -> a real "
        "change would silently reuse a stale lowering (miscompile risk)"
    )
    # And the hydrated form of one must not collide with the other either.
    hydrated_a = _digest(_round_trip(_payload_with_ast(node_a)))
    assert hydrated_a == digest_a
    assert hydrated_a != digest_b


def test_binary_integer_ast_and_default_metadata_keep_cache_provenance_identity() -> (
    None
):
    previous = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(640)
        value = (1 << 20000) + 1
        fresh = _payload_with_ast(ast.parse("0x1" + "0" * 5000, mode="eval"))
        fresh["known_func_defaults"] = {
            "pkg.mod": {"f": {"defaults": [{"const": True, "value": value}]}}
        }
        hydrated = _round_trip(fresh)
        assert (
            hydrated["known_func_defaults"]["pkg.mod"]["f"]["defaults"][0]["value"]
            == value
        )
        assert (
            type(
                hydrated["known_func_defaults"]["pkg.mod"]["f"]["defaults"][0]["value"]
            )
            is int
        )
        assert _digest(fresh) is not None
        assert _digest(fresh) == _digest(hydrated)
        hydrated["known_func_defaults"]["pkg.mod"]["f"]["defaults"][0]["value"] += 1
        assert _digest(fresh) != _digest(hydrated)
    finally:
        sys.set_int_max_str_digits(previous)


def test_persisted_cache_admission_rejects_noncanonical_integer_metadata(
    tmp_path: Path,
) -> None:
    source = tmp_path / "mod.py"
    source.write_text("def f(value=0x100):\n    return value\n", encoding="utf-8")
    source_lease = _ModuleSourceLease.path_backed(source)
    source_metadata = {
        "size": source.stat().st_size,
        "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
    }
    # Independent signed-byte fixture: 1 followed by 256 bytes is 2**2048.
    valid = {"__integer_bytes__": [1] + [0] * 256}
    invalid = [
        {"__integer_bytes__": []},
        {"__integer_bytes__": [0, 1]},
        {"__integer_bytes__": [255, 255]},
        {"__integer_bytes__": [True]},
        {"__integer_bytes__": [256]},
        {"__integer_bytes__": "01"},
    ]
    previous = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(640)
        for raw in [valid, *invalid]:
            analysis = {
                **source_metadata,
                "version": MC._MODULE_ANALYSIS_CACHE_SCHEMA_VERSION,
                "compiler_fingerprint": MC._frontend_semantic_tooling_fingerprint(),
                "import_scan_mode": "module_init",
                "body_selection": {"kind": "complete_source", "roots": []},
                "func_kinds": {"f": "sync"},
                "func_defaults": {
                    "f": {
                        "kind": "sync",
                        "has_decorators": False,
                        "params": 1,
                        "posonly": 0,
                        "kwonly": 0,
                        "defaults": [{"const": True, "value": raw}],
                    }
                },
            }
            lowering = {
                **source_metadata,
                "version": MC._MODULE_LOWERING_CACHE_SCHEMA_VERSION,
                "source_kind": "path",
                "context_digest": "context",
                "result": {
                    "body_selection": {"kind": "complete_source", "roots": []},
                    "functions": [],
                    "function_metadata": {},
                    "constant_metadata": raw,
                },
            }
            # Validate rich metadata JSON; lowering bodies use a separate pure codec.
            analysis_result = MC._validate_persisted_module_analysis_payload(
                json.loads(json.dumps(analysis)),
                source,
                import_scan_mode="module_init",
                path_stat=source.stat(),
            )
            lowering_result = MC._validate_persisted_module_lowering_payload(
                json.loads(json.dumps(lowering)),
                source,
                body_selection=ModuleBodySelection("complete_source"),
                source_lease=source_lease,
                module_name="mod",
                context_digest="context",
            )
            if raw is valid:
                assert analysis_result is not None
                assert lowering_result is not None
                assert analysis_result[0]["f"]["defaults"][0]["value"] == 1 << 2048
                assert lowering_result["constant_metadata"] == 1 << 2048
            else:
                assert analysis_result is None
                assert lowering_result is None
    finally:
        sys.set_int_max_str_digits(previous)


def test_source_producers_round_trip_tag_names_and_large_literal_container_keys(
    tmp_path: Path,
) -> None:
    from molt.cli.frontend_worker import _module_frontend_payload
    from molt.frontend import SimpleTIRGenerator
    from molt.frontend.sema.funcmeta import collect_module_func_kinds
    from molt.opcode_literal_payloads import integer_literal_value

    huge_hex = "0x1" + "0" * 600
    ordered = f"{{{huge_hex}: {{'__integer_bytes__': [1]}}, 1: 'last'}}"
    reversed_order = f"{{1: 'last', {huge_hex}: {{'__integer_bytes__': [1]}}}}"
    named = "{'__integer_bytes__': [1], 'plain': 2}"
    reversed_named = "{'plain': 2, '__integer_bytes__': [1]}"
    text = (
        f"def __integer_bytes__(value={huge_hex}, "
        f"mapping={ordered}, members={{{huge_hex}, 'text'}}, "
        f"named={named}):\n    return value\n"
        f"class __dict__:\n    mapping = {ordered}\n    named = {named}\n"
    )
    source = tmp_path / "mod.py"
    source.write_text(text, encoding="utf-8")
    source_lease = _ModuleSourceLease.path_backed(source)
    source_metadata = {
        "size": source.stat().st_size,
        "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
    }
    previous = sys.get_int_max_str_digits()
    try:
        sys.set_int_max_str_digits(640)
        tree = ast.parse(text)
        defaults = MC._collect_func_defaults(tree)
        kinds = collect_module_func_kinds(tree)
        specs = defaults["__integer_bytes__"]["defaults"]
        assert specs == [
            {"const": True, "value": 1 << 2400},
            {"const": False},
            {"const": False},
            {"const": False},
        ]
        analysis = {
            **source_metadata,
            "version": MC._MODULE_ANALYSIS_CACHE_SCHEMA_VERSION,
            "compiler_fingerprint": MC._frontend_semantic_tooling_fingerprint(),
            "import_scan_mode": "module_init",
            "body_selection": {"kind": "complete_source", "roots": []},
            "func_defaults": defaults,
            "func_kinds": kinds,
        }
        generator = SimpleTIRGenerator(module_source_role="user")
        generator.visit(tree)
        ir = generator.to_json()
        result = _module_frontend_payload(
            generator,
            ir,
            source_sha256=source_lease.source_sha256,
            visit_s=0.0,
            lower_s=0.0,
            total_s=0.0,
        )
        tag_named_class = ClassId("__main__", "__dict__")
        assert tag_named_class in result["local_classes"]
        assert any(
            integer_literal_value(bytes(op["bytes"])) == 1 << 2400
            for function in result["functions"]
            for op in function["ops"]
            if op["kind"] == "const_bigint"
        )
        lowering = {
            **source_metadata,
            "version": MC._MODULE_LOWERING_CACHE_SCHEMA_VERSION,
            "source_kind": "path",
            "context_digest": "context",
            "result": result,
        }

        def persisted(payload: dict[str, object]) -> object:
            return json.loads(
                json.dumps(CK._cache_json_value(payload), default=CK._json_ir_default)
            )

        analysis_result = MC._validate_persisted_module_analysis_payload(
            persisted(analysis),
            source,
            import_scan_mode="module_init",
            path_stat=source.stat(),
        )
        assert analysis_result == (defaults, kinds)
        lowering_result = MC._validate_persisted_module_lowering_payload(
            persisted(lowering),
            source,
            body_selection=ModuleBodySelection("complete_source"),
            source_lease=source_lease,
            module_name="__main__",
            context_digest="context",
        )
        assert lowering_result is not None
        assert list(lowering_result["local_classes"]) == [tag_named_class]
        assert lowering_result["functions"] == result["functions"]
        assert CK._backend_ir_bytes({"functions": lowering_result["functions"]}) == (
            CK._backend_ir_bytes({"functions": result["functions"]})
        )
        assert _digest(result) == _digest(lowering_result)
        assert _digest(result) is not None
        changed = dict(
            lowering_result,
            local_classes={
                ClassId("__main__", "__integer_bytes__"): lowering_result[
                    "local_classes"
                ][tag_named_class]
            },
        )
        assert _digest(result) != _digest(changed)
        for original, reordered in ((ordered, reversed_order), (named, reversed_named)):
            other = SimpleTIRGenerator(module_source_role="user")
            other.visit(ast.parse(text.replace(original, reordered)))
            other_ir = other.to_json()
            assert other_ir["functions"] != result["functions"]
            assert CK._cache_payload_digest(other_ir) != CK._cache_payload_digest(ir)
    finally:
        sys.set_int_max_str_digits(previous)
