from __future__ import annotations

from .errors import OpKindTableError
from .runtime_requirements import registered_runtime_kinds
from .schema import _CLASSIFIER_SETS, _SIMPLEIR_FIELD_ROLE_FACT_SETS


def _shared_simpleir_kinds(data: dict) -> set[str]:
    registered = registered_runtime_kinds(data)
    for table in (*_CLASSIFIER_SETS, *_SIMPLEIR_FIELD_ROLE_FACT_SETS):
        registered.update(data.get(table, ()))
    registered.update(data.get("simpleir_preserved_kinds", ()))
    return registered


def backend_private_kinds(data: dict) -> dict[str, str]:
    return {
        kind: row["backend"]
        for row in data.get("simpleir_backend_private_kinds", ())
        for kind in row["kinds"]
    }


def registered_simpleir_kinds(data: dict) -> set[str]:
    return _shared_simpleir_kinds(data) | backend_private_kinds(data).keys()


def registered_frontend_kinds(data: dict) -> set[str]:
    registered = {kind.upper() for kind in _shared_simpleir_kinds(data)}
    for table in (
        "frontend_effect_kind",
        "frontend_raising_kind",
        "frontend_check_exception_skip",
        "frontend_lowering_kind",
    ):
        registered.update(row["kind"] for row in data.get(table, ()))
    for row in data.get("kind", ()):
        if row.get("group") == "gpu":
            registered.update((row["canonical"], *row.get("aliases", ())))
    return registered


def validate_registration(data: dict) -> None:
    preserved = data.get("simpleir_preserved_kinds", [])
    if (
        not isinstance(preserved, list)
        or any(not isinstance(kind, str) or not kind for kind in preserved)
        or len(set(preserved)) != len(preserved)
    ):
        raise OpKindTableError("simpleir_preserved_kinds requires unique kind strings")
    private_rows = data.get("simpleir_backend_private_kinds", [])
    if not isinstance(private_rows, list):
        raise OpKindTableError(
            "simpleir_backend_private_kinds requires backend/kinds rows"
        )
    claimed = _shared_simpleir_kinds(data)
    backends: set[str] = set()
    for row in private_rows:
        if not isinstance(row, dict) or set(row) != {"backend", "kinds"}:
            raise OpKindTableError(
                "simpleir_backend_private_kinds requires backend/kinds rows"
            )
        backend, kinds = row["backend"], row["kinds"]
        if (
            not isinstance(backend, str)
            or not backend.isidentifier()
            or backend in backends
        ):
            raise OpKindTableError(
                "simpleir_backend_private_kinds requires unique backend identifiers"
            )
        backends.add(backend)
        if (
            not isinstance(kinds, list)
            or not kinds
            or any(
                not isinstance(kind, str) or not kind.isidentifier() for kind in kinds
            )
            or len(set(kinds)) != len(kinds)
            or claimed.intersection(kinds)
        ):
            raise OpKindTableError(
                "simpleir_backend_private_kinds requires unique unclaimed kinds"
            )
        claimed.update(kinds)
    # Frontend lowering targets must belong to the shared wire vocabulary.
    wire_kinds = _shared_simpleir_kinds(data)
    seen: set[str] = set()
    for row in data.get("frontend_lowering_kind", ()):
        if not isinstance(row, dict) or set(row) != {"kind", "wire_kind"}:
            raise OpKindTableError("frontend_lowering_kind requires kind and wire_kind")
        kind = row["kind"]
        if not isinstance(kind, str) or not kind.isidentifier() or kind in seen:
            raise OpKindTableError(
                "frontend_lowering_kind requires unique kind identifiers"
            )
        seen.add(kind)
        if not isinstance(row["wire_kind"], str) or row["wire_kind"] not in wire_kinds:
            raise OpKindTableError(
                f"frontend_lowering_kind {kind}: unregistered wire kind {row['wire_kind']!r}"
            )
