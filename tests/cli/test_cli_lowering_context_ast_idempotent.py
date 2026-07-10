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

import ast
import importlib
import json

CK = importlib.import_module("molt.cli.cache_keys")
MC = importlib.import_module("molt.cli.module_cache")


def _round_trip(payload: dict[str, object]) -> object:
    """Encode ``payload`` the way the caches do, then decode it back.

    Mirrors the real persistence path: ``json.dumps(..., default=_json_ir_default)``
    on write and :func:`_decode_cached_json_value` on read. The decoded object is
    what a warm cross-session build holds in memory for a cache-hydrated module.
    """
    encoded = json.dumps(payload, sort_keys=True, default=CK._json_ir_default)
    return MC._decode_cached_json_value(json.loads(encoded))


def _digest(payload: dict[str, object]) -> str | None:
    return MC._module_lowering_context_digest(payload)


def _payload_with_ast(node: ast.AST) -> dict[str, object]:
    # A minimal stand-in for the class-metadata shape that carries AST values on
    # the lowering context path (``known_classes`` -> class -> inline_init_assigns).
    return {
        "module_name": "pkg.mod",
        "known_classes": {
            "C": {
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
