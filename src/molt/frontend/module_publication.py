"""Symbol-keyed frontend assembly facts, separate from executable functions."""

from __future__ import annotations

from collections.abc import Collection, Mapping
from dataclasses import dataclass
from typing import Any, TypedDict, cast


class SourceModulePublication(TypedDict):
    """Backend-assembly facts owned by the source module-init prologue."""

    module_name: str
    module_value: str
    failure_label: int


class SourceModulePublicationData(TypedDict):
    publication: SourceModulePublication
    boundary_index: int


class FunctionAssemblyMetadata(TypedDict, total=False):
    source_module_publication: SourceModulePublicationData
    gpu_body_origin: dict[str, Any]
    app_callable_bindings: list[dict[str, Any]]


_FUNCTION_METADATA_FIELDS = frozenset(FunctionAssemblyMetadata.__annotations__)


@dataclass(frozen=True)
class SourceModulePublicationEnvelope:
    """Validated publication metadata and its single operation boundary."""

    publication: SourceModulePublication
    boundary_index: int


def parse_source_module_publication(payload: object) -> SourceModulePublication:
    """Validate the assembly-only identity and frame publication payload."""
    if not isinstance(payload, Mapping) or set(payload) != {
        "module_name",
        "module_value",
        "failure_label",
    }:
        raise ValueError("missing canonical source-module publication metadata")
    module_name = payload.get("module_name")
    module_value = payload.get("module_value")
    failure_label = payload.get("failure_label")
    if (
        not isinstance(module_name, str)
        or not module_name
        or not isinstance(module_value, str)
        or not module_value
        or type(failure_label) is not int
    ):
        raise ValueError("missing canonical source-module publication metadata")
    return SourceModulePublication(
        module_name=module_name, module_value=module_value, failure_label=failure_label
    )


def reject_embedded_function_metadata(function: Mapping[str, Any]) -> None:
    if _FUNCTION_METADATA_FIELDS.intersection(function):
        raise ValueError(
            f"function {function.get('name')!r} contains legacy embedded assembly metadata"
        )


def reject_embedded_operation_metadata(operation: Mapping[str, Any]) -> None:
    if "source_module_publication_boundary" in operation:
        raise ValueError("operation contains legacy source_module_publication_boundary")


def parse_function_metadata(
    payload: object, *, function_symbols: Collection[str]
) -> dict[str, FunctionAssemblyMetadata]:
    """Admit the closed symbol-keyed sidecar without copying its rows or bodies."""
    if not isinstance(payload, dict):
        raise ValueError("missing canonical function_metadata sidecar")
    for symbol, row in payload.items():
        if not isinstance(symbol, str) or symbol not in function_symbols:
            raise ValueError(f"function metadata has no executable owner {symbol!r}")
        if not isinstance(row, dict) or set(row) - _FUNCTION_METADATA_FIELDS:
            raise ValueError(f"invalid assembly metadata for function {symbol!r}")
        if "source_module_publication" in row:
            envelope = row["source_module_publication"]
            if (
                not isinstance(envelope, dict)
                or set(envelope) != {"publication", "boundary_index"}
                or type(envelope.get("boundary_index")) is not int
                or envelope["boundary_index"] < 0
            ):
                raise ValueError(
                    f"invalid source-module publication envelope for {symbol!r}"
                )
            parse_source_module_publication(envelope["publication"])
        if "gpu_body_origin" in row and not isinstance(row["gpu_body_origin"], dict):
            raise ValueError(f"invalid GPU body origin for function {symbol!r}")
        if "app_callable_bindings" in row and not isinstance(
            row["app_callable_bindings"], list
        ):
            raise ValueError(f"invalid app callable bindings for function {symbol!r}")
    return cast(dict[str, FunctionAssemblyMetadata], payload)


def inspect_source_module_publication(
    function: Mapping[str, Any], metadata: Mapping[str, Any]
) -> SourceModulePublicationEnvelope | None:
    """Validate the explicit boundary against the original executable body."""
    reject_embedded_function_metadata(function)
    if "source_module_publication" not in metadata:
        return None
    raw = metadata["source_module_publication"]
    name = repr(function.get("name"))
    if not isinstance(raw, Mapping) or set(raw) != {"publication", "boundary_index"}:
        raise ValueError(f"source module init {name} has invalid publication envelope")
    publication = parse_source_module_publication(raw["publication"])
    boundary_index = raw["boundary_index"]
    ops = function.get("ops")
    if (
        type(boundary_index) is not int
        or boundary_index < 0
        or not isinstance(ops, list)
        or boundary_index >= len(ops)
    ):
        raise ValueError(
            f"source module init {name} has invalid publication boundary index"
        )
    boundary_op = ops[boundary_index]
    if (
        not isinstance(boundary_op, Mapping)
        or boundary_op.get("kind") != "frame_locals_set"
    ):
        raise ValueError(
            f"source module init {name} publication boundary lost its frame-locals anchor"
        )
    reject_embedded_operation_metadata(boundary_op)
    return SourceModulePublicationEnvelope(publication, boundary_index)


def project_frontend_tir_to_executable(tir: Mapping[str, Any]) -> dict[str, Any]:
    """Validate the frontend sidecar and project the same executable records."""
    functions = tir.get("functions")
    if not isinstance(functions, list):
        raise ValueError("frontend IR functions must be a list")
    symbols: set[str] = set()
    for function in functions:
        if not isinstance(function, dict) or not isinstance(function.get("name"), str):
            raise ValueError("frontend IR function must have a symbol")
        reject_embedded_function_metadata(function)
        if function["name"] in symbols:
            raise ValueError(
                f"duplicate executable function owner {function['name']!r}"
            )
        symbols.add(function["name"])
        for op in function.get("ops", []):
            reject_embedded_operation_metadata(op)
    metadata = parse_function_metadata(
        tir.get("function_metadata"), function_symbols=symbols
    )
    for function in functions:
        inspect_source_module_publication(function, metadata.get(function["name"], {}))
    return {key: value for key, value in tir.items() if key != "function_metadata"}
