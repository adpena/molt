"""The typed class model: one record per class statement.

A class statement's semantic facts (bases, MRO, instance layout, members,
constructor plan, dataclass spec) are settled when its module finishes
lowering: the statement fixes the layout and bindings, and each method adds its
compiled summary (shape, inline body) as the module compiles it. While the
module lowers, the facts live in a mutable ``ClassDraft``; at publication the
draft seals into a frozen ``ClassModel``. Only the model crosses a module
boundary (worker payloads, the module cache, integration's program-wide class
table).

What lowering alone needs (the class's SSA values, the methods still pending)
stays on the draft and is never sealed.

``resolve`` is the one member lookup through an MRO. Every refusal is a typed
value with derived text, so a consumer that cannot use a fact names why in a
build diagnostic without restating the rule.
"""

from __future__ import annotations

import ast
from collections.abc import Callable
from dataclasses import dataclass, field
from enum import Enum
from typing import TYPE_CHECKING, Protocol

from molt.frontend._types import BuiltinClass, ClassId, ClassRef, TypeHint
from molt.frontend.dataclass_init import DataclassInitField

if TYPE_CHECKING:
    from molt.frontend._types import MethodInfo


@dataclass(frozen=True, slots=True)
class Shape:
    """A def's parameter shape, as a call binds it."""

    params: int
    posonly: int
    kwonly: int
    defaults: int
    vararg: bool
    varkw: bool

    @property
    def binds_nothing(self) -> bool:
        """An exact positional call binds nothing: no defaults, no keyword-only
        parameters, no ``*args`` or ``**kwargs``."""
        return not (self.defaults or self.kwonly or self.vararg or self.varkw)


class Closure(Enum):
    """What a method's closure holds."""

    NONE = "none"
    # Only ``__class__``: written once when the class statement runs.
    CLASS_CELL = "class_cell"
    # Any other free variable: lexical state a proven entry must carry.
    CELLS = "cells"


class MemberKind(Enum):
    """How the class binds a name defined in its body."""

    FUNCTION = "function"
    CLASSMETHOD = "classmethod"
    STATICMETHOD = "staticmethod"
    PROPERTY = "property"
    PROPERTY_UPDATE = "property_update"
    # A def under another decorator: its value is what the decorator returns.
    DECORATED = "decorated"
    # A class attribute that is not a def.
    DATA = "data"


@dataclass(frozen=True, slots=True)
class InlineBody:
    """A body small enough to splice at a call: ``return <expr>`` over its
    parameters, or field stores for ``__init__``.

    ``source`` (the unparsed body) is its identity. The AST nodes exist only in
    the process that lowered the method: a model decoded from the cache keeps
    the source and no AST, so it is not inlined (the cache codec never carried
    source positions)."""

    params: tuple[str, ...]
    source: str
    returns: ast.expr | None = field(default=None, compare=False, hash=False)
    # None: no field-store summary; empty: an ``__init__`` that stores none.
    init_assigns: tuple[tuple[str, ast.expr], ...] | None = field(
        default=None, compare=False, hash=False
    )

    @classmethod
    def of(
        cls,
        params: tuple[str, ...],
        returns: ast.expr | None,
        init_assigns: tuple[tuple[str, ast.expr], ...] | None,
    ) -> InlineBody:
        """The body with its source identity: an exact structural encoding of
        its expressions (``ast_identity``)."""
        source = ast_identity(
            (
                returns,
                None
                if init_assigns is None
                else tuple((name, expr) for name, expr in init_assigns),
            )
        )
        return cls(params, source, returns, init_assigns)


def ast_identity(value: object) -> str:
    """An exact, deterministic text of an AST value: node types and fields in
    order, integers in hex. ``ast.unparse`` and ``ast.dump`` render integers in
    decimal, which raises past ``sys.get_int_max_str_digits()``; hex has no
    such limit."""
    if isinstance(value, ast.AST):
        fields = ",".join(
            f"{name}={ast_identity(getattr(value, name, None))}"
            for name in value._fields
        )
        return f"{type(value).__name__}({fields})"
    if isinstance(value, (list, tuple)):
        return "[" + ",".join(ast_identity(item) for item in value) + "]"
    if type(value) is int:
        return hex(value)
    return repr(value)


@dataclass(frozen=True, slots=True)
class Member:
    """One name the class body binds."""

    kind: MemberKind
    # The compiled function's symbol for a def; None for data.
    symbol: str | None = None
    shape: Shape | None = None
    closure: Closure = Closure.NONE
    returns_none: bool = False
    inline: InlineBody | None = None
    # A property's field, and for a setter/deleter which update it is.
    property_field: str | None = None
    property_update: str | None = None


class LayoutRefusal(Enum):
    """Why a class has no static instance layout."""

    METACLASS = "a metaclass builds it"
    MULTIPLE_BASES = "it has several bases"
    NATIVE_BASE = "a native base owns its payload prefix"
    SLOTS = "it declares __slots__"
    ATTRIBUTE_HOOKS = "it defines attribute hooks"
    VISIBLE_MUTATION = "code mutates the class"
    OPEN_BASE = "a base is not namespace-closed"
    DYNAMIC_BUILD = "the statement builds dynamically"
    OBSERVABLE_BODY = "its body can run a callback or reflect on its namespace"


@dataclass(frozen=True, slots=True)
class Field:
    name: str
    offset: int
    hint: TypeHint | None


@dataclass(frozen=True, slots=True)
class InstanceLayout:
    """The instance fields inferred for the class, at their offsets in
    declaration order, and the byte size. ``refusal`` is None when the offsets
    are the instance's static layout; otherwise it names why the runtime owns
    the layout, and the fields stay inferred names and hints only."""

    fields: tuple[Field, ...]
    size: int
    refusal: LayoutRefusal | None

    @property
    def static(self) -> bool:
        return self.refusal is None

    def field(self, name: str) -> Field | None:
        for item in self.fields:
            if item.name == name:
                return item
        return None

    def offset(self, name: str) -> int | None:
        item = self.field(name)
        return None if item is None else item.offset


@dataclass(frozen=True, slots=True)
class DataclassSpec:
    """A ``@dataclass`` statement's literal options and, when every base's is
    known, the fields its generated ``__init__`` binds (a subclass extends its
    bases' projections)."""

    params: tuple[tuple[str, bool], ...]
    init_projection: tuple[DataclassInitField, ...] | None

    def option(self, name: str) -> bool:
        return dict(self.params).get(name, False)


@dataclass(frozen=True, slots=True)
class ConstructorPlan:
    """``C(args)`` allocates inline and runs ``__init__`` directly."""


@dataclass(frozen=True, slots=True)
class ConstructorRefusal:
    reason: str


type Constructor = ConstructorPlan | ConstructorRefusal


class ClassView(Protocol):
    """The facts every reader may use, from a draft or a sealed model."""

    @property
    def class_id(self) -> ClassId: ...
    @property
    def binding_name(self) -> str: ...
    @property
    def bases(self) -> tuple[ClassRef, ...]: ...
    @property
    def linearization(self) -> tuple[ClassRef, ...]: ...
    @property
    def layout(self) -> InstanceLayout: ...
    @property
    def module_level(self) -> bool: ...
    @property
    def definition_exposed(self) -> bool: ...
    @property
    def binding_proven(self) -> bool: ...
    @property
    def custom_metaclass(self) -> bool: ...
    @property
    def decorated(self) -> bool: ...
    @property
    def layout_version(self) -> int: ...
    @property
    def possible_data_descriptors(self) -> frozenset[str]: ...
    @property
    def dataclass(self) -> DataclassSpec | None: ...
    @property
    def constructor(self) -> Constructor: ...

    def member(self, name: str) -> Member | None: ...

    def pending(self, name: str) -> bool:
        """The body defines ``name`` as a def its module has not compiled
        yet; always False on a sealed model."""
        ...


@dataclass(frozen=True, slots=True)
class ClassModel:
    """The sealed facts of one class statement."""

    class_id: ClassId
    binding_name: str
    bases: tuple[ClassRef, ...]
    linearization: tuple[ClassRef, ...]
    layout: InstanceLayout
    module_level: bool
    # Code the program cannot see may hold the class or one of its methods.
    definition_exposed: bool
    # No code the closed program holds rebinds the class name or a member
    # (a closed namespace premise and an unexposed definition).
    binding_proven: bool
    custom_metaclass: bool
    decorated: bool
    # The layout version the runtime computes for the class; guards compare it.
    layout_version: int
    members: tuple[tuple[str, Member], ...]
    # Class-level names the defining module cannot prove are not data
    # descriptors (a value that may define ``__set__``, ``__delete__`` or
    # ``__new__``, or a binding the body makes some other way).
    possible_data_descriptors: frozenset[str]
    dataclass: DataclassSpec | None
    constructor: Constructor

    def member(self, name: str) -> Member | None:
        for member_name, member in self.members:
            if member_name == name:
                return member
        return None

    def pending(self, name: str) -> bool:
        return False

    def cache_record(self) -> dict[str, object]:
        """The model as a cache record: primitives, tuples, class references
        and type hints (which the cache codec owns), and expressions as their
        source text, so ``from_cache_record`` rebuilds an equal model."""
        return {
            "class_id": self.class_id,
            "binding_name": self.binding_name,
            "bases": self.bases,
            "linearization": self.linearization,
            "layout": (
                tuple((f.name, f.offset, f.hint) for f in self.layout.fields),
                self.layout.size,
                None if self.layout.refusal is None else self.layout.refusal.name,
            ),
            "module_level": self.module_level,
            "definition_exposed": self.definition_exposed,
            "binding_proven": self.binding_proven,
            "custom_metaclass": self.custom_metaclass,
            "decorated": self.decorated,
            "layout_version": self.layout_version,
            "members": tuple(
                (name, _member_record(member)) for name, member in self.members
            ),
            "possible_data_descriptors": tuple(sorted(self.possible_data_descriptors)),
            "dataclass": None
            if self.dataclass is None
            else (
                self.dataclass.params,
                None
                if self.dataclass.init_projection is None
                else tuple(f.schema() for f in self.dataclass.init_projection),
            ),
            "constructor": None
            if isinstance(self.constructor, ConstructorPlan)
            else self.constructor.reason,
        }

    @classmethod
    def from_cache_record(cls, record: dict[str, object]) -> ClassModel:
        """The inverse of ``cache_record``; a malformed record raises."""
        fields, size, refusal = record["layout"]  # type: ignore[misc]
        dataclass_record = record["dataclass"]
        constructor = record["constructor"]
        return cls(
            class_id=record["class_id"],  # type: ignore[arg-type]
            binding_name=record["binding_name"],  # type: ignore[arg-type]
            bases=tuple(record["bases"]),  # type: ignore[arg-type]
            linearization=tuple(record["linearization"]),  # type: ignore[arg-type]
            layout=InstanceLayout(
                tuple(Field(name, offset, hint) for name, offset, hint in fields),
                size,
                None if refusal is None else LayoutRefusal[refusal],
            ),
            module_level=record["module_level"],  # type: ignore[arg-type]
            definition_exposed=record["definition_exposed"],  # type: ignore[arg-type]
            binding_proven=record["binding_proven"],  # type: ignore[arg-type]
            custom_metaclass=record["custom_metaclass"],  # type: ignore[arg-type]
            decorated=record["decorated"],  # type: ignore[arg-type]
            layout_version=record["layout_version"],  # type: ignore[arg-type]
            members=tuple(
                (name, _member_from_record(member))
                for name, member in record["members"]  # type: ignore[union-attr]
            ),
            possible_data_descriptors=frozenset(record["possible_data_descriptors"]),  # type: ignore[arg-type]
            dataclass=None
            if dataclass_record is None
            else DataclassSpec(
                params=tuple(tuple(item) for item in dataclass_record[0]),  # type: ignore[index]
                init_projection=None
                if dataclass_record[1] is None  # type: ignore[index]
                else tuple(
                    DataclassInitField.from_schema(schema)
                    for schema in dataclass_record[1]  # type: ignore[index]
                ),
            ),
            constructor=ConstructorPlan()
            if constructor is None
            else ConstructorRefusal(constructor),  # type: ignore[arg-type]
        )



def _member_record(member: Member) -> tuple[object, ...]:
    shape = member.shape
    inline = member.inline
    return (
        member.kind.name,
        member.symbol,
        None
        if shape is None
        else (
            shape.params,
            shape.posonly,
            shape.kwonly,
            shape.defaults,
            shape.vararg,
            shape.varkw,
        ),
        member.closure.name,
        member.returns_none,
        None if inline is None else (inline.params, inline.source),
        member.property_field,
        member.property_update,
    )


def _member_from_record(record: tuple[object, ...]) -> Member:
    kind, symbol, shape, closure, returns_none, inline, field_name, update = record
    return Member(
        kind=MemberKind[kind],  # type: ignore[index]
        symbol=symbol,  # type: ignore[arg-type]
        shape=None if shape is None else Shape(*shape),  # type: ignore[misc]
        closure=Closure[closure],  # type: ignore[index]
        returns_none=returns_none,  # type: ignore[arg-type]
        inline=None
        if inline is None
        else InlineBody(params=tuple(inline[0]), source=inline[1]),  # type: ignore[index]
        property_field=field_name,  # type: ignore[arg-type]
        property_update=update,  # type: ignore[arg-type]
    )


@dataclass(slots=True)
class ClassDraft:
    """A class statement's facts while its body lowers.

    The statement fixes everything but the members; the body's bindings run in
    source order through ``bind_member``, ``bind_data`` and ``unbind``, and
    ``seal`` returns the frozen model when the statement completes. Until a
    binding runs, its name is pending, and a lookup that reaches it refuses.
    """

    class_id: ClassId
    binding_name: str
    bases: tuple[ClassRef, ...]
    linearization: tuple[ClassRef, ...]
    layout: InstanceLayout
    module_level: bool
    definition_exposed: bool
    # No code the closed program holds rebinds the class name or a member
    # (a closed namespace premise and an unexposed definition).
    binding_proven: bool
    custom_metaclass: bool
    decorated: bool
    layout_version: int
    # Class-level names the defining module cannot prove are not data
    # descriptors (a value that may define ``__set__``, ``__delete__`` or
    # ``__new__``, or a binding the body makes some other way).
    possible_data_descriptors: frozenset[str]
    dataclass: DataclassSpec | None
    constructor: Constructor = ConstructorRefusal(
        "the class statement has not finished (a call in its own body)"
    )
    members: dict[str, Member] = field(default_factory=dict)
    # Names the body binds whose binding has not run yet.
    pending_members: set[str] = field(default_factory=set)

    def member(self, name: str) -> Member | None:
        return self.members.get(name)

    def pending(self, name: str) -> bool:
        return name in self.pending_members

    def bind_member(self, name: str, member: Member) -> None:
        """A def bound ``name`` to ``member`` (a property setter or deleter
        binds the property it extends)."""
        self.members[name] = member
        self.pending_members.discard(name)

    def bind_data(self, name: str) -> None:
        """A class-level store bound ``name`` to a value that is not a def."""
        self.members[name] = Member(MemberKind.DATA)
        self.pending_members.discard(name)

    def unbind(self, name: str) -> None:
        """``del name`` in the body: lookups continue to the bases."""
        self.members.pop(name, None)
        self.pending_members.discard(name)

    def seal(self) -> ClassModel:
        return ClassModel(
            class_id=self.class_id,
            binding_name=self.binding_name,
            bases=self.bases,
            linearization=self.linearization,
            layout=self.layout,
            module_level=self.module_level,
            definition_exposed=self.definition_exposed,
            binding_proven=self.binding_proven,
            custom_metaclass=self.custom_metaclass,
            decorated=self.decorated,
            layout_version=self.layout_version,
            members=tuple(sorted(self.members.items())),
            possible_data_descriptors=self.possible_data_descriptors,
            dataclass=self.dataclass,
            constructor=self.constructor,
        )


def namespace_closed(view: ClassView) -> bool:
    """Whether unguarded facts about the class namespace hold (HF-812).

    A layout guard checks the receiver's own class at its layout version, and
    every mutation of the class or a base advances that version, so guarded
    field access needs only a static layout. Unguarded facts also need an
    unexposed definition: allocation, method binding, method bodies and
    unguarded field access all assume that no unseen code rebinds a member or
    replaces a method body."""
    return view.layout.static and not view.definition_exposed


class ResolveRefusal(Enum):
    PENDING = "the class defines it later in its body"
    SHADOWED_BY_DATA = "a class attribute that is not a def shadows it"
    UNKNOWN_CLASS = "a class on the MRO has no model"
    NOT_FOUND = "no class on the MRO defines it"


@dataclass(frozen=True, slots=True)
class Resolved:
    member: Member
    owner: ClassId


@dataclass(frozen=True, slots=True)
class Unresolved:
    reason: ResolveRefusal
    owner: ClassId | None = None


type Resolution = Resolved | Unresolved


def resolve(
    mro: tuple[ClassRef, ...],
    name: str,
    lookup: Callable[[ClassId], ClassView | None],
    *,
    after: ClassId | None = None,
) -> Resolution:
    """The member ``name`` resolves to through ``mro``.

    ``after`` starts the walk past that class, as ``super()`` inside one of its
    methods does. A builtin class on the MRO is skipped: its members are the
    runtime's, which no class model describes. A def the body defines later
    (``pending``) or a data attribute that is not a def stops the walk.
    """
    walk = mro
    if after is not None:
        if after not in mro:
            return Unresolved(ResolveRefusal.NOT_FOUND)
        walk = mro[mro.index(after) + 1 :]
    for owner in walk:
        if isinstance(owner, BuiltinClass):
            continue
        view = lookup(owner)
        if view is None:
            return Unresolved(ResolveRefusal.UNKNOWN_CLASS, owner)
        member = view.member(name)
        if member is None and view.pending(name):
            return Unresolved(ResolveRefusal.PENDING, owner)
        if member is None:
            continue
        if member.kind is MemberKind.DATA:
            return Unresolved(ResolveRefusal.SHADOWED_BY_DATA, owner)
        return Resolved(member, owner)
    return Unresolved(ResolveRefusal.NOT_FOUND)


def member_from_method_info(info: MethodInfo) -> Member:
    """The one translation of a compiled method's lowering record into its
    class member fact. ``func``'s type hint names the compiled symbol
    (``Func:<symbol>``); the SSA values stay with the defining module."""
    hint = info["func"].type_hint
    symbol = (
        hint.split(":", 1)[1]
        if isinstance(hint, str) and hint.startswith("Func:")
        else None
    )
    inline_params = info.get("inline_params")
    inline = None
    if inline_params is not None:
        returns = info.get("inline_return")
        assigns = info.get("inline_init_assigns")
        init_assigns = tuple(assigns) if assigns is not None else None
        inline = InlineBody.of(tuple(inline_params), returns, init_assigns)
    return Member(
        kind=MemberKind(info["descriptor"]),
        symbol=symbol,
        shape=Shape(
            params=info["param_count"],
            posonly=info["posonly_count"],
            kwonly=info["kwonly_count"],
            defaults=len(info["defaults"]),
            vararg=info["has_vararg"],
            varkw=info["has_varkw"],
        ),
        closure=Closure.CELLS if info["has_closure"] else Closure.NONE,
        returns_none=bool(info.get("returns_none")),
        inline=inline,
        property_field=info.get("property_field"),
        property_update=info.get("property_update"),
    )


def defines(view: ClassView, name: str) -> bool:
    """Whether the class body binds ``name``: a def, data, or a binding that
    has not run yet."""
    return view.member(name) is not None or view.pending(name)
