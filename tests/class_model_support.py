"""Typed class models for tests that place a class in a generator's table.

A test states only the facts it exercises; every other fact takes the value a
class statement without that feature would seal (a static layout, an
unexposed definition, no members, no dataclass, a refused constructor).
"""

from __future__ import annotations

from collections.abc import Mapping

from molt.frontend import ClassId
from molt.frontend._types import OBJECT_CLASS, ClassRef, TypeHint
from molt.frontend.class_model import (
    ClassModel,
    ConstructorRefusal,
    DataclassSpec,
    Field,
    InstanceLayout,
    LayoutRefusal,
    Member,
    MemberKind,
    Shape,
)
from molt.frontend.dataclass_init import DataclassInitField


def plain_method(symbol: str, *, params: int = 1) -> Member:
    """A positional-only def compiled to ``symbol`` (``self`` included)."""
    return Member(
        MemberKind.FUNCTION,
        symbol=symbol,
        shape=Shape(params, 0, 0, 0, False, False),
    )


def class_model(
    class_id: ClassId,
    *,
    binding_name: str | None = None,
    linearization: tuple[ClassRef, ...] | None = None,
    fields: Mapping[str, int] | None = None,
    hints: Mapping[str, TypeHint] | None = None,
    size: int | None = None,
    dynamic: bool = False,
    exposed: bool = False,
    module_level: bool = False,
    binding_proven: bool = False,
    layout_version: int = 0,
    members: Mapping[str, Member] | None = None,
    dataclass_projection: tuple[DataclassInitField, ...] | None = None,
    dataclass: bool = False,
) -> ClassModel:
    fields = dict(fields or {})
    hints = dict(hints or {})
    linearization = linearization or (class_id, OBJECT_CLASS)
    return ClassModel(
        class_id=class_id,
        binding_name=binding_name or class_id.name,
        bases=linearization[1:2],
        linearization=linearization,
        layout=InstanceLayout(
            tuple(
                Field(name, offset, hints.get(name)) for name, offset in fields.items()
            ),
            8 * (len(fields) + 1) if size is None else size,
            LayoutRefusal.MULTIPLE_BASES if dynamic else None,
        ),
        module_level=module_level,
        definition_exposed=exposed,
        binding_proven=binding_proven,
        custom_metaclass=False,
        decorated=False,
        layout_version=layout_version,
        members=tuple(sorted((members or {}).items())),
        possible_data_descriptors=frozenset(),
        dataclass=(
            DataclassSpec((), dataclass_projection)
            if dataclass or dataclass_projection is not None
            else None
        ),
        constructor=ConstructorRefusal("test model"),
    )
