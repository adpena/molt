"""The lowering context digest must ignore lowering-time derived class memos.

A scoped ``known_classes`` snapshot carries per-class metadata that includes
LAZILY-POPULATED derived memos -- most notably ``exception_subclass``
(:mod:`molt.frontend.lowering.class_resolution`), a bool written onto the shared
``class_info`` the first time some module's lowering calls
``_class_is_exception_subclass``. It is derived entirely from the class's
``mro``/bases (already in the digest) plus the builtin-exception name set (in the
frontend tooling fingerprint), so it carries no independent information.

Because it is populated lazily during lowering, a class whose defining module was
a CACHE HIT (its lowering never ran this session) lacks the memo, while a
fresh-lowered class has it. When that class is an ubiquitous base
(``collections.abc.Iterable`` / ``Mapping`` / ``Sized`` ...), every downstream
module that scopes it then hashed a *different* ``known_classes`` snapshot
depending on cache-population state -- the residual that kept the numpy witness
cross-session lowering-cache hit rate at ~0.82 after the idempotent-AST fix
(26/152 modules missed, all on the ``known_classes`` field, with byte-identical
lowered output).

These tests pin the fix as the two-direction miscompile-safety gate (#39 /
CODEX-B pattern):

* **provenance-invariance -> HIT:** a class_info with the memo present and the
  same class_info with it absent (or set to either bool) produce the SAME context
  digest, so a fresh-lowered and a cache-hydrated snapshot hash identically.
* **semantic sensitivity -> MISS:** a genuinely different class definition (e.g.
  different ``bases``/``mro``) still changes the digest, so a real class change
  invalidates the persisted lowering rather than reusing a stale one.
"""

from __future__ import annotations

import importlib

MC = importlib.import_module("molt.cli.module_cache")


def _digest_with_known_classes(known_classes: dict) -> str | None:
    # The context payload the shared per-module lowering cache keys on. Only the
    # known_classes field varies across these cases; everything else is fixed.
    payload = {
        "version": 2,
        "module_name": "pkg.consumer",
        "known_classes": MC._known_classes_for_context_digest(known_classes),
    }
    return MC._module_lowering_context_digest(payload)


def _base_class_info(**overrides) -> dict:
    info = {
        "module": "collections.abc",
        "bases": ["object"],
        "mro": ["Iterable", "object"],
        "methods": {},
        "fields": {},
        "size": 8,
    }
    info.update(overrides)
    return info


def test_exception_subclass_memo_presence_does_not_change_digest() -> None:
    """Memo present vs absent -> SAME digest (the fresh-vs-hydrated case)."""
    hydrated = {"Iterable": _base_class_info()}  # cache hit: memo never populated
    fresh = {"Iterable": _base_class_info(exception_subclass=False)}  # fresh: memo set

    d_hydrated = _digest_with_known_classes(hydrated)
    d_fresh = _digest_with_known_classes(fresh)

    assert d_hydrated is not None
    assert d_hydrated == d_fresh, (
        "context digest depends on the lazily-populated exception_subclass memo -> "
        "a cache-hydrated known_classes snapshot hashes differently than a "
        "fresh-lowered one and the cross-session lowering cache misses"
    )


def test_exception_subclass_memo_value_does_not_change_digest() -> None:
    """True vs False vs absent all collapse to one digest (derived, not semantic)."""
    absent = {"E": _base_class_info(module="mod", mro=["E", "Exception", "object"])}
    memo_true = {
        "E": _base_class_info(
            module="mod", mro=["E", "Exception", "object"], exception_subclass=True
        )
    }
    memo_false = {
        "E": _base_class_info(
            module="mod", mro=["E", "Exception", "object"], exception_subclass=False
        )
    }
    digests = {
        _digest_with_known_classes(absent),
        _digest_with_known_classes(memo_true),
        _digest_with_known_classes(memo_false),
    }
    assert len(digests) == 1, (
        "exception_subclass memo value leaks into the context digest; it is a "
        "derived cache, not a semantic input"
    )


def test_real_class_change_still_changes_digest() -> None:
    """Direction 2: a genuine class-definition change MUST change the digest.

    The projection only drops the derived memo; the semantic fields (bases/mro/
    methods/...) that actually drive lowering still participate, so a real change
    cannot silently reuse a stale lowering.
    """
    base = {"C": _base_class_info(bases=["object"], mro=["C", "object"])}
    changed_bases = {"C": _base_class_info(bases=["Base"], mro=["C", "Base", "object"])}
    changed_methods = {"C": _base_class_info(methods={"f": {"param_count": 0}})}

    d0 = _digest_with_known_classes(base)
    assert d0 is not None
    assert d0 != _digest_with_known_classes(changed_bases), (
        "changed bases/mro collapsed to the same digest -> a real class change "
        "would silently reuse a stale lowering (miscompile risk)"
    )
    assert d0 != _digest_with_known_classes(changed_methods)


def test_projection_does_not_mutate_shared_class_info() -> None:
    """The digest projection must never strip the memo from the live class_info.

    The lowering path relies on the memo as a real cache; only the digest view
    drops it. Projecting must leave the caller's dict untouched.
    """
    live = _base_class_info(exception_subclass=False)
    known = {"Iterable": live}
    _ = MC._known_classes_for_context_digest(known)
    assert live.get("exception_subclass") is False
    assert "exception_subclass" in live
