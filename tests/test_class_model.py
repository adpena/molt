"""The class model's member lookup against CPython's own MRO lookup.

The oracle is the interpreter running these tests: each case builds real
classes, and the expected owner of ``name`` (past ``after`` for ``super()``)
is the first class on ``cls.__mro__`` whose ``__dict__`` binds it.
"""

from __future__ import annotations

import pytest

from molt.frontend._types import ClassId
from molt.frontend.class_model import (
    ClassModel,
    ClassView,
    ConstructorRefusal,
    DynamicLayout,
    LayoutRefusal,
    Member,
    MemberKind,
    Resolved,
    ResolveRefusal,
    Unresolved,
    resolve,
)


def _hierarchy() -> dict[str, type]:
    class A:
        def m(self) -> str:
            return "A"

        tag = "data"

    class B(A):
        def m(self) -> str:
            return "B"

    class C(A):
        pass

    class D(B, C):
        pass

    class E(C):
        def tag(self) -> str:
            return "E"

    class F(D, E):
        tag = 1

    return {cls.__name__: cls for cls in (A, B, C, D, E, F)}


_CLASSES = _hierarchy()
_IDS = {name: ClassId("m", name) for name in _CLASSES}


def _model(cls: type) -> ClassModel:
    members = []
    for name, value in vars(cls).items():
        if name.startswith("__"):
            continue
        kind = MemberKind.FUNCTION if callable(value) else MemberKind.DATA
        members.append((name, Member(kind, symbol=f"m__{cls.__name__}_{name}")))
    linearization = tuple(_IDS[k.__name__] for k in cls.__mro__ if k is not object)
    return ClassModel(
        class_id=_IDS[cls.__name__],
        binding_name=cls.__name__,
        bases=tuple(_IDS[b.__name__] for b in cls.__bases__ if b is not object),
        linearization=linearization,
        layout=DynamicLayout(LayoutRefusal.MULTIPLE_BASES, 0),
        module_level=True,
        definition_exposed=False,
        custom_metaclass=False,
        decorated=False,
        layout_version=1,
        members=tuple(sorted(members)),
        field_defaults=(),
        dataclass=None,
        constructor=ConstructorRefusal("test"),
    )


_MODELS = {_IDS[name]: _model(cls) for name, cls in _CLASSES.items()}


def _lookup(class_id: ClassId) -> ClassView | None:
    return _MODELS.get(class_id)


def _cpython_owner(cls: type, name: str, after: type | None) -> type | None:
    mro = cls.__mro__
    if after is not None:
        mro = mro[mro.index(after) + 1 :]
    for klass in mro:
        if klass is not object and name in vars(klass):
            return klass
    return None


_CASES = [
    (cls, name, after)
    for cls in _CLASSES.values()
    for name in ("m", "tag", "missing")
    for after in (None, *(k for k in cls.__mro__ if k is not object))
]


@pytest.mark.parametrize(
    "cls,name,after",
    _CASES,
    ids=[f"{c.__name__}.{n}@{a.__name__ if a else '-'}" for c, n, a in _CASES],
)
def test_resolve_matches_the_interpreters_mro_lookup(
    cls: type, name: str, after: type | None
) -> None:
    model = _MODELS[_IDS[cls.__name__]]
    result = resolve(
        model.linearization,
        name,
        _lookup,
        after=_IDS[after.__name__] if after is not None else None,
    )
    owner = _cpython_owner(cls, name, after)
    if owner is None:
        assert result == Unresolved(ResolveRefusal.NOT_FOUND)
    elif callable(vars(owner)[name]):
        assert isinstance(result, Resolved)
        assert result.owner == _IDS[owner.__name__]
        assert result.member.symbol == f"m__{owner.__name__}_{name}"
    else:
        assert result == Unresolved(
            ResolveRefusal.SHADOWED_BY_DATA, _IDS[owner.__name__]
        )


def test_a_def_the_body_has_not_compiled_stops_the_walk() -> None:
    from molt.frontend.class_model import ClassDraft

    draft = ClassDraft(
        class_id=_IDS["B"],
        binding_name="B",
        bases=(_IDS["A"],),
        linearization=(_IDS["B"], _IDS["A"]),
        layout=DynamicLayout(LayoutRefusal.MULTIPLE_BASES, 0),
        module_level=True,
        definition_exposed=False,
        custom_metaclass=False,
        decorated=False,
        layout_version=1,
        field_defaults=(),
        dataclass=None,
        pending_members={"m"},
    )
    lookup = {_IDS["B"]: draft, _IDS["A"]: _MODELS[_IDS["A"]]}.get
    pending = resolve(draft.linearization, "m", lookup)
    assert pending == Unresolved(ResolveRefusal.PENDING, _IDS["B"])
    member = Member(MemberKind.FUNCTION, symbol="m__B_m")
    draft.compile_member("m", member)
    assert resolve(draft.linearization, "m", lookup) == Resolved(member, _IDS["B"])
    sealed = draft.seal()
    assert sealed.member("m") == member and not sealed.pending("m")


def test_a_class_without_a_model_refuses_rather_than_skipping() -> None:
    lookup = {_IDS["D"]: _MODELS[_IDS["D"]]}.get
    result = resolve(_MODELS[_IDS["D"]].linearization, "m", lookup)
    assert result == Unresolved(ResolveRefusal.UNKNOWN_CLASS, _IDS["B"])


def test_a_sealed_model_is_immutable_and_hashable() -> None:
    model = _MODELS[_IDS["A"]]
    with pytest.raises(AttributeError):
        model.binding_name = "Z"  # type: ignore[misc]
    assert hash(model) == hash(_model(_CLASSES["A"]))
