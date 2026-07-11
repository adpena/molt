"""The frontend-cache value encoder must be a decode fixed point (all types).

Root invariant behind the cross-session frontend-lowering cache hit rate: the
per-module lowering *context digest* is a sha256 over ``json.dumps(payload,
default=_json_ir_default)``. A module's context payload is assembled from values
that may be held in either of two provably-equivalent representations:

* the *fresh* form a module analysed this build produces (a live ``ast.AST``, a
  ``tuple``, a ``set``/``frozenset``, ``bytes``, ``complex``, ``Ellipsis``, a
  ``MoltValue``); or
* the *hydrated* form the analysis/lowering cache decoder
  (:func:`_decode_cached_json_value`) yields after a cross-session round-trip.

For the digest to depend only on the source (not on which modules happened to be
cache-hydrated this run), the encode/decode round-trip must be a **fixed point**
for every value the encoder can emit::

    encode(x) == encode(decode(encode(x)))

The historical ``ast.AST`` miscompile-safety bug (encoder wrote ``{"__ast__": s}``
while the decoder produced the bare string ``s`` -> non-idempotent -> witness
lowering-cache hit_rate ~0.19; see test_cli_lowering_context_ast_idempotent) was
one instance of this invariant breaking. This gate pins the invariant for the
WHOLE encoder so a *future* wrapper type (or a regression to an existing one)
that breaks the round-trip is caught here instead of silently collapsing the
cross-session cache hit rate again.

Two directions (the #39 / CODEX-B miscompile-safety pattern):

* **Fixed point (representation-invariance -> HIT):** every encoder-emitted form
  re-encodes byte-identically after a decode round-trip, so a fresh payload and a
  cache-hydrated payload hash to the same context digest.
* **Injectivity (real change -> MISS):** genuinely different values keep distinct
  encodings, so a real semantic change still invalidates the persisted lowering
  rather than reusing a stale one.
"""

from __future__ import annotations

import ast
import importlib
import json

CK = importlib.import_module("molt.cli.cache_keys")
MC = importlib.import_module("molt.cli.module_cache")
MoltValue = importlib.import_module("molt.frontend").MoltValue


def _enc(value: object) -> str:
    """Encode exactly as the frontend caches do (sorted, compact, IR default)."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"), default=CK._json_ir_default)


def _round_trip(value: object) -> object:
    """The in-memory value a cross-session cache-hydrated payload holds."""
    return MC._decode_cached_json_value(json.loads(_enc(value)))


# A live ast.AST node of the shape carried on the lowering context path
# (class ``inline_init_assigns`` hold AST-bearing metadata).
_NODE = ast.parse("optionalRelease + 1", mode="eval").body

# Every value type ``_json_ir_default`` can emit, plus nestings that occur in
# real context payloads (known_func_defaults, known_classes, type_facts).
_FIXED_POINT_CASES: dict[str, object] = {
    "complex": complex(1.5, -2.0),
    "ellipsis": ...,
    "bytes": b"\x00\x01hello\xff",
    "tuple": (1, 2, "x"),
    "nested_tuple": (1, (2, 3), ("a", ("b",))),
    "ast": _NODE,
    "set": {3, 1, 2},
    "frozenset": frozenset({"b", "a", "c"}),
    "molt_value": MoltValue(name="v", type_hint="int"),
    "tuple_of_ast": (_NODE, "s"),
    "set_in_tuple": (frozenset({1, 2}), 3),
    "func_defaults_shape": {
        "f": {"a": (1, 2), "b": frozenset({1, 2}), "c": b"xy", "d": 3 + 4j, "e": ...}
    },
    "known_classes_shape": {
        "C": {"methods": {"__init__": {"inline_init_assigns": [["attr", _NODE]]}}}
    },
    "list_of_mixed": [(1, 2), {3, 4}, _NODE, b"z"],
}


def test_encoder_is_a_decode_fixed_point_for_every_type() -> None:
    """encode(x) == encode(decode(encode(x))) for every encoder-emitted form.

    A single failing case means a fresh-analysis payload and a cache-hydrated
    payload hash to different context digests for a module carrying that value,
    so the cross-session shared lowering cache silently stops hitting (the
    ~0.19-hit-rate class of regression).
    """
    broken: list[str] = []
    for name, value in _FIXED_POINT_CASES.items():
        once = _enc(value)
        twice = _enc(_round_trip(value))
        if once != twice:
            broken.append(f"{name}: {once!r} != {twice!r}")
    assert not broken, (
        "frontend-cache value encoder is NOT a decode fixed point for: "
        + "; ".join(broken)
        + " -- a cache-hydrated lowering context hashes differently than a fresh "
        "one, so the cross-session shared lowering cache will miss on every module "
        "carrying that value (configured != effective, M34)."
    )


def test_context_digest_is_provenance_invariant_over_full_payload() -> None:
    """A full context-shaped payload hashes identically fresh vs hydrated.

    Exercises the real digest entry point (:func:`_module_lowering_context_digest`)
    over a payload that carries AST class metadata, tuple/set/bytes/complex
    function defaults, and a type-facts tuple -- the fields whose representation
    differs between a freshly-analysed and a cache-hydrated module.
    """
    payload = {
        "module_name": "pkg.mod",
        "known_classes": _FIXED_POINT_CASES["known_classes_shape"],
        "known_func_defaults": _FIXED_POINT_CASES["func_defaults_shape"],
        "type_facts": {"modules": {"pkg.mod": (1, 2, 3)}},
        "native_python_exports": frozenset({"a", "b"}),
    }
    fresh = MC._module_lowering_context_digest(payload)
    hydrated = MC._module_lowering_context_digest(_round_trip(payload))
    assert fresh is not None
    assert fresh == hydrated, (
        "context digest depends on cache provenance over the full payload -> the "
        "cross-session shared lowering cache cannot hit"
    )


def test_encoder_is_injective_for_distinct_values() -> None:
    """Direction 2: genuinely different values keep distinct encodings.

    Guards against a 'fix' that makes the round-trip idempotent by collapsing
    distinct values together (which would silently reuse a stale lowering -- a
    miscompile). Each pair below is a real semantic difference that MUST change
    the encoding.
    """
    distinct_pairs = [
        ((1, 2), (1, 3)),
        (frozenset({1, 2}), frozenset({1, 2, 3})),
        (b"xy", b"xz"),
        (complex(1, 2), complex(1, 3)),
        (ast.parse("a", mode="eval").body, ast.parse("b", mode="eval").body),
        (MoltValue(name="v", type_hint="int"), MoltValue(name="v", type_hint="str")),
        ({1, 2}, (1, 2)),  # a set and a tuple of the same members are NOT the same
    ]
    for left, right in distinct_pairs:
        assert _enc(left) != _enc(right), f"encoder collapsed distinct values: {left!r} == {right!r}"
        # And the collapse must not appear after a round-trip either.
        assert _enc(_round_trip(left)) != _enc(_round_trip(right))
