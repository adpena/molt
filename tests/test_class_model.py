"""The class model's member lookup against CPython's own MRO lookup.

The oracle is the interpreter running these tests: each case builds real
classes, and the expected owner of ``name`` (past ``after`` for ``super()``)
is the first class on ``cls.__mro__`` whose ``__dict__`` binds it.
"""

from __future__ import annotations

import pytest

from molt.frontend._types import ClassId
from molt.frontend.class_model import (
    ClassDraft,
    ClassModel,
    ClassView,
    ConstructorPlan,
    ConstructorRefusal,
    Field,
    InstanceLayout,
    LayoutRefusal,
    Member,
    MemberKind,
    ReleasePlan,
    ReleaseRefusal,
    Resolved,
    ResolveRefusal,
    Unresolved,
    release_plan,
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
        layout=InstanceLayout((), 0, LayoutRefusal.MULTIPLE_BASES),
        module_level=True,
        definition_exposed=False,
        binding_proven=True,
        custom_metaclass=False,
        decorated=False,
        layout_version=1,
        members=tuple(sorted(members)),
        possible_data_descriptors=frozenset(),
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
    draft = ClassDraft(
        class_id=_IDS["B"],
        binding_name="B",
        bases=(_IDS["A"],),
        linearization=(_IDS["B"], _IDS["A"]),
        layout=InstanceLayout((), 0, LayoutRefusal.MULTIPLE_BASES),
        module_level=True,
        definition_exposed=False,
        binding_proven=True,
        custom_metaclass=False,
        decorated=False,
        layout_version=1,
        possible_data_descriptors=frozenset(),
        dataclass=None,
        pending_members={"m"},
    )
    lookup = {_IDS["B"]: draft, _IDS["A"]: _MODELS[_IDS["A"]]}.get
    pending = resolve(draft.linearization, "m", lookup)
    assert pending == Unresolved(ResolveRefusal.PENDING, _IDS["B"])
    member = Member(MemberKind.FUNCTION, symbol="m__B_m")
    draft.bind_member("m", member)
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


_OPERATIONS = ("def", "data", "property", "setter", "del")


def _body_source(sequence: tuple[str, ...]) -> str:
    lines = []
    for operation in sequence:
        if operation == "def":
            lines.append("def m(self): return 1")
        elif operation == "data":
            lines.append("m = 2")
        elif operation == "property":
            lines.append("@property\ndef m(self): return 3")
        elif operation == "setter":
            # A setter extends the property `m` names; anything else raises.
            lines.append(
                "if isinstance(globals().get('m', locals().get('m')), property):\n"
                "    @m.setter\n"
                "    def m(self, value): pass"
            )
        else:
            lines.append("if 'm' in locals():\n    del m")
    return "\n".join(lines) or "pass"


def _apply(
    draft: ClassDraft, sequence: tuple[str, ...], namespace: dict[str, str]
) -> None:
    # Mirror the body: each operation runs only when CPython's guard runs it.
    for operation in sequence:
        if operation == "def":
            draft.bind_member("m", Member(MemberKind.FUNCTION, symbol="def"))
            namespace["m"] = "function"
        elif operation == "data":
            draft.bind_data("m")
            namespace["m"] = "data"
        elif operation == "property":
            draft.bind_member("m", Member(MemberKind.PROPERTY, symbol="prop"))
            namespace["m"] = "property"
        elif operation == "setter":
            # The setter's def binds the property it extends (the lowering
            # reads the previous member before its store).
            previous = draft.member("m")
            if namespace.get("m") == "property" and previous is not None:
                draft.bind_member("m", previous)
        elif "m" in namespace:
            draft.unbind("m")
            del namespace["m"]


import itertools as _itertools  # noqa: E402

_SEQUENCES = [
    seq
    for length in range(1, 4)
    for seq in _itertools.product(_OPERATIONS, repeat=length)
]


@pytest.mark.parametrize("sequence", _SEQUENCES, ids=["-".join(s) for s in _SEQUENCES])
def test_body_bindings_in_order_match_the_interpreter(
    sequence: tuple[str, ...],
) -> None:
    class Base:
        def m(self) -> int:
            return 0

    body = _body_source(sequence)
    namespace: dict = {}
    exec(body, {}, namespace)
    cls = type("C", (Base,), namespace)
    draft = ClassDraft(
        class_id=ClassId("t", "C"),
        binding_name="C",
        bases=(ClassId("t", "Base"),),
        linearization=(ClassId("t", "C"), ClassId("t", "Base")),
        layout=InstanceLayout((), 0, None),
        module_level=True,
        definition_exposed=False,
        binding_proven=True,
        custom_metaclass=False,
        decorated=False,
        layout_version=1,
        possible_data_descriptors=frozenset(),
        dataclass=None,
        pending_members={"m"},
    )
    _apply(draft, sequence, {})
    base = ClassModel(
        class_id=ClassId("t", "Base"),
        binding_name="Base",
        bases=(),
        linearization=(ClassId("t", "Base"),),
        layout=InstanceLayout((), 0, None),
        module_level=True,
        definition_exposed=False,
        binding_proven=True,
        custom_metaclass=False,
        decorated=False,
        layout_version=1,
        members=(("m", Member(MemberKind.FUNCTION, symbol="base")),),
        possible_data_descriptors=frozenset(),
        dataclass=None,
        constructor=ConstructorRefusal("test"),
    )
    lookup = {draft.class_id: draft, ClassId("t", "Base"): base}.get
    draft.pending_members.clear()  # the statement has completed
    result = resolve(draft.linearization, "m", lookup)
    own = vars(cls).get("m")
    if own is None:
        assert isinstance(result, Resolved) and result.owner == ClassId("t", "Base")
    elif isinstance(own, property):
        assert isinstance(result, Resolved)
        assert result.member.kind is MemberKind.PROPERTY
        assert result.owner == draft.class_id
    elif callable(own):
        assert isinstance(result, Resolved)
        assert result.member.kind is MemberKind.FUNCTION
    else:
        assert result == Unresolved(ResolveRefusal.SHADOWED_BY_DATA, draft.class_id)


def _layout_model(layout: InstanceLayout, *, metaclass: bool = False) -> ClassModel:
    return ClassModel(
        class_id=ClassId("t", "Point"),
        binding_name="Point",
        bases=(),
        linearization=(ClassId("t", "Point"),),
        layout=layout,
        module_level=True,
        definition_exposed=False,
        binding_proven=True,
        custom_metaclass=metaclass,
        decorated=False,
        layout_version=1,
        members=(),
        possible_data_descriptors=frozenset(),
        dataclass=None,
        constructor=ConstructorPlan(),
    )


_POINT_LAYOUT = InstanceLayout((Field("x", 0, None), Field("y", 8, "int")), 24, None)


@pytest.mark.parametrize(
    "layout,metaclass,expected",
    [
        # The runtime's plain instance: fields from offset 0, then the
        # dictionary word (molt-runtime `lifecycle_plan::plain_field_words`).
        (_POINT_LAYOUT, False, ReleasePlan(2)),
        (InstanceLayout((), 8, None), False, ReleasePlan(0)),
        # A gap, a missing dictionary word, a dict-subclass tail, a refused
        # layout and a metaclass all leave the plan to the run time.
        (InstanceLayout((Field("x", 8, None),), 24, None), False, None),
        (InstanceLayout((Field("x", 0, None),), 8, None), False, None),
        (InstanceLayout((Field("x", 0, None),), 24, None), False, None),
        (InstanceLayout((), 8, LayoutRefusal.SLOTS), False, None),
        (_POINT_LAYOUT, True, None),
    ],
)
def test_release_plan_follows_the_runtime_plain_layout(
    layout: InstanceLayout, metaclass: bool, expected: ReleasePlan | None
) -> None:
    plan = release_plan(_layout_model(layout, metaclass=metaclass))
    if expected is None:
        assert isinstance(plan, ReleaseRefusal) and plan.reason
    else:
        assert plan == expected


def test_release_plan_survives_the_cache_record() -> None:
    model = _layout_model(_POINT_LAYOUT)
    restored = ClassModel.from_cache_record(model.cache_record())
    assert restored == model
    assert release_plan(restored) == release_plan(model) == ReleasePlan(2)
