"""One compact wire projection of CAS-owned execution custody detail."""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path

from molt.exact_json import canonical_json_sha256
from tools.proof_queue_pkg import custody_cas


DETAIL_KIND = "execution-receipt-details"
PROJECTION_SCHEMA = "molt.proof-execution-detail-projection.v1"
CONTEXT_LIMIT_BYTES = 64 * 1024

# These are inventories, not admission decisions. Their complete values are
# restored and checked by the existing consumers; no list is sampled or clipped.
_DETAIL_PATHS = (
    ("child_process_custody", "policy", "allowed"),
    ("child_process_custody", "receipt", "events"),
    ("child_process_custody", "receipt", "errors"),
    ("child_process_custody", "receipt", "violations"),
    ("execution_environment", "prelaunch", "variables"),
    ("execution_environment", "prelaunch", "passed_names"),
    ("execution_environment", "prelaunch", "omitted_names"),
    ("execution_environment", "prelaunch", "override_names"),
    ("execution_environment", "prelaunch", "cargo_policies"),
    ("custody_authorities", "prelaunch"),
)


def _present_fields(context: Mapping[str, object]) -> dict[str, object]:
    fields: dict[str, object] = {}
    for path in _DETAIL_PATHS:
        value: object = context
        for key in path:
            if not isinstance(value, Mapping) or key not in value:
                break
            value = value[key]
        else:
            fields["/".join(path)] = value
    return fields


def _projection(value: object) -> dict[str, object]:
    if not isinstance(value, (list, dict)):
        raise ValueError("execution receipt detail must be an object or array")
    return {
        "schema": PROJECTION_SCHEMA,
        "kind": "object" if isinstance(value, dict) else "array",
        "count": len(value),
        "sha256": canonical_json_sha256(value),
    }


def _require_binding(context: Mapping[str, object]) -> None:
    run_id = context.get("run_id")
    nonce = context.get("execution_nonce_sha256")
    if (
        not isinstance(run_id, str)
        or not run_id
        or not isinstance(nonce, str)
        or len(nonce) != 64
        or any(char not in "0123456789abcdef" for char in nonce)
    ):
        raise ValueError("execution receipt detail requires a run/nonce binding")


def _replace_fields(
    context: Mapping[str, object], fields: Mapping[str, object]
) -> dict[str, object]:
    # Copy only modified ancestors: large unrelated CAS projections and native
    # supervisor authority keep their existing ownership and identity.
    result = dict(context)
    for pointer, value in fields.items():
        path = pointer.split("/")
        node = result
        for key in path[:-1]:
            parent = node.get(key)
            if not isinstance(parent, Mapping):
                raise ValueError(
                    f"execution receipt detail parent is absent: {pointer}"
                )
            child = dict(parent)
            node[key] = child
            node = child
        node[path[-1]] = value
    return result


def compact_context(
    context: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    """Publish complete inventory detail and return its one wire representation."""
    if "execution_details" in context:
        raise ValueError("execution receipt context is already compact")
    _require_binding(context)
    fields = _present_fields(context)
    projections = {pointer: _projection(value) for pointer, value in fields.items()}
    artifact = custody_cas.put_json(
        cas_root,
        {
            "schema": custody_cas.ARTIFACT_SCHEMA,
            "kind": DETAIL_KIND,
            "run_id": context.get("run_id"),
            "execution_nonce_sha256": context.get("execution_nonce_sha256"),
            "fields": fields,
        },
    ).as_dict()
    compact = _replace_fields(context, projections)
    compact["execution_details"] = artifact
    return compact


def expand_context(
    context: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    """Verify detail bytes and exact projection closure before full admission."""
    _require_binding(context)
    reference = context.get("execution_details")
    if not isinstance(reference, Mapping):
        raise ValueError("execution receipt has no durable detail authority")
    payload = custody_cas.read_ref(reference, expected_root=cas_root)
    if (
        payload.get("schema") != custody_cas.ARTIFACT_SCHEMA
        or payload.get("kind") != DETAIL_KIND
        or payload.get("run_id") != context.get("run_id")
        or payload.get("execution_nonce_sha256")
        != context.get("execution_nonce_sha256")
    ):
        raise ValueError("execution receipt detail run/nonce binding is invalid")
    fields = payload.get("fields")
    projections = _present_fields(context)
    if not isinstance(fields, dict) or set(fields) != set(projections):
        raise ValueError("execution receipt detail field closure is invalid")
    for pointer, value in fields.items():
        projection = projections[pointer]
        if (
            not isinstance(projection, dict)
            or type(projection.get("count")) is not int
            or projection != _projection(value)
        ):
            raise ValueError(f"execution receipt detail projection differs: {pointer}")
    expanded = _replace_fields(context, fields)
    expanded.pop("execution_details")
    return expanded
