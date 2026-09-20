"""Source-module publication facts at the frontend/assembly phase boundary."""

from __future__ import annotations

from collections.abc import Mapping, MutableMapping
from copy import deepcopy
from dataclasses import dataclass
from typing import Any, TypedDict


class SourceModulePublication(TypedDict):
    """Backend-assembly facts owned by the source module-init prologue."""

    module_name: str
    module_value: str
    failure_label: int


@dataclass(frozen=True)
class SourceModulePublicationEnvelope:
    """Validated publication metadata and its single operation boundary."""

    publication: SourceModulePublication
    boundary_index: int


def parse_source_module_publication(payload: object) -> SourceModulePublication:
    """Validate the assembly-only identity and frame publication payload."""
    if not isinstance(payload, Mapping):
        raise ValueError("missing canonical source-module publication metadata")
    fields: Mapping[Any, object] = payload
    if set(fields) != {"module_name", "module_value", "failure_label"}:
        raise ValueError("missing canonical source-module publication metadata")
    module_name = fields.get("module_name")
    module_value = fields.get("module_value")
    failure_label = fields.get("failure_label")
    if (
        not isinstance(module_name, str)
        or not module_name
        or not isinstance(module_value, str)
        or not module_value
        or not isinstance(failure_label, int)
        or isinstance(failure_label, bool)
    ):
        raise ValueError("missing canonical source-module publication metadata")
    return SourceModulePublication(
        module_name=module_name,
        module_value=module_value,
        failure_label=failure_label,
    )


def _function_label(function: Mapping[str, object]) -> str:
    name = function.get("name")
    return repr(name) if isinstance(name, str) else "<unnamed>"


def inspect_source_module_publication(
    function: Mapping[str, object],
) -> SourceModulePublicationEnvelope | None:
    """Validate one complete publication envelope without consuming it."""
    has_publication = "source_module_publication" in function
    ops = function.get("ops")
    boundaries: list[tuple[int, Mapping[Any, object]]] = []
    if isinstance(ops, list):
        for index, op in enumerate(ops):
            if not isinstance(op, Mapping):
                continue
            operation: Mapping[Any, object] = op
            if "source_module_publication_boundary" not in operation:
                continue
            if operation["source_module_publication_boundary"] is not True:
                raise ValueError(
                    f"source module init {_function_label(function)} publication "
                    "boundary marker must be true"
                )
            boundaries.append((index, operation))

    if not has_publication:
        if boundaries:
            raise ValueError(
                f"source module init {_function_label(function)} has an unowned "
                "publication boundary"
            )
        return None

    publication = parse_source_module_publication(
        function.get("source_module_publication")
    )
    if not isinstance(ops, list):
        raise ValueError(
            f"source module init {_function_label(function)} publication ops must "
            "be a list"
        )
    if len(boundaries) != 1:
        raise ValueError(
            f"source module init {_function_label(function)} must retain exactly "
            "one native publication boundary"
        )
    boundary_index, boundary_op = boundaries[0]
    if boundary_op.get("kind") != "frame_locals_set":
        raise ValueError(
            f"source module init {_function_label(function)} publication boundary "
            "lost its frame-locals anchor"
        )
    return SourceModulePublicationEnvelope(publication, boundary_index)


def consume_source_module_publication(
    function: MutableMapping[str, object],
) -> SourceModulePublicationEnvelope | None:
    """Validate and remove one frontend-only publication envelope."""
    envelope = inspect_source_module_publication(function)
    if envelope is None:
        return None
    ops = function["ops"]
    assert isinstance(ops, list)
    boundary_op = ops[envelope.boundary_index]
    if not isinstance(boundary_op, MutableMapping):
        raise TypeError("source module publication boundary must be mutable")
    boundary: MutableMapping[Any, Any] = boundary_op
    function.pop("source_module_publication")
    boundary.pop("source_module_publication_boundary")
    return envelope


def project_frontend_tir_to_executable(tir: Mapping[str, Any]) -> dict[str, Any]:
    """Copy frontend assembly IR and consume its non-executable envelopes."""
    projected = deepcopy(dict(tir))
    functions = projected.get("functions")
    if not isinstance(functions, list):
        return projected
    for function in functions:
        if isinstance(function, MutableMapping):
            consume_source_module_publication(function)
    return projected
