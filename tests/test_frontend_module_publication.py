from __future__ import annotations

from collections import UserDict
from types import MappingProxyType

import pytest

from molt.frontend.module_publication import (
    inspect_source_module_publication,
    parse_source_module_publication,
    project_frontend_tir_to_executable,
)


def _publication() -> dict[str, object]:
    return {
        "module_name": "unit",
        "module_value": "module",
        "failure_label": 7,
    }


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("module_name", ""),
        ("module_name", None),
        ("module_name", 1),
        ("module_value", ""),
        ("module_value", None),
        ("module_value", 1),
        ("failure_label", None),
        ("failure_label", "7"),
        ("failure_label", True),
        ("failure_label", False),
    ],
)
def test_publication_fields_are_validated_before_typed_projection(
    field: str, value: object
) -> None:
    payload = _publication()
    payload[field] = value
    with pytest.raises(
        ValueError, match="canonical source-module publication metadata"
    ):
        parse_source_module_publication(payload)


def test_publication_accepts_mapping_protocol_without_coercing_fields() -> None:
    payload = _publication()
    assert parse_source_module_publication(UserDict(payload)) == payload
    assert parse_source_module_publication(MappingProxyType(payload)) == payload
    with pytest.raises(
        ValueError, match="canonical source-module publication metadata"
    ):
        parse_source_module_publication({**payload, 0: "extra"})


def test_publication_inspection_preserves_immutable_body_and_sidecar() -> None:
    boundary = MappingProxyType({"kind": "frame_locals_set", "args": ["module"]})
    function = MappingProxyType(
        {"name": "molt_init_unit", "params": [], "ops": [boundary]}
    )
    metadata = {
        "source_module_publication": {
            "publication": _publication(),
            "boundary_index": 0,
        }
    }
    envelope = inspect_source_module_publication(function, metadata)
    assert envelope is not None
    assert envelope.publication == _publication()
    assert envelope.boundary_index == 0
    assert function["ops"][0] is boundary
    assert dict(boundary) == {"kind": "frame_locals_set", "args": ["module"]}
    assert metadata == {
        "source_module_publication": {
            "publication": _publication(),
            "boundary_index": 0,
        }
    }


def test_executable_projection_retains_bodies_and_drops_only_sidecar() -> None:
    function = {
        "name": "molt_init_unit",
        "params": [],
        "ops": [{"kind": "frame_locals_set", "args": ["module"]}],
    }
    metadata = {
        "molt_init_unit": {
            "source_module_publication": {
                "publication": _publication(),
                "boundary_index": 0,
            }
        }
    }
    document = {
        "functions": [function],
        "function_metadata": metadata,
        "code_slots": {"molt_init_unit": 0},
    }
    projected = project_frontend_tir_to_executable(document)
    assert "function_metadata" not in projected
    assert projected["functions"] is document["functions"]
    assert projected["functions"][0] is function
    assert document["function_metadata"] is metadata
    assert projected["code_slots"] is document["code_slots"]


@pytest.mark.parametrize(
    "defect",
    [
        "missing",
        "orphan",
        "duplicate",
        "unknown-field",
        "embedded",
        "negative-index",
        "bool-index",
        "past-end",
        "wrong-anchor",
    ],
)
def test_executable_projection_rejects_invalid_sidecar_ownership(defect: str) -> None:
    function = {
        "name": "molt_init_unit",
        "params": [],
        "ops": [{"kind": "frame_locals_set", "args": ["module"]}],
    }
    envelope = {"publication": _publication(), "boundary_index": 0}
    metadata = {"molt_init_unit": {"source_module_publication": envelope}}
    document = {"functions": [function], "function_metadata": metadata}
    if defect == "missing":
        del document["function_metadata"]
    elif defect == "orphan":
        metadata["absent"] = metadata.pop("molt_init_unit")
    elif defect == "duplicate":
        document["functions"].append(dict(function))
    elif defect == "unknown-field":
        metadata["molt_init_unit"]["unknown"] = True
    elif defect == "embedded":
        function["source_module_publication"] = _publication()
    elif defect == "negative-index":
        envelope["boundary_index"] = -1
    elif defect == "bool-index":
        envelope["boundary_index"] = True
    elif defect == "past-end":
        envelope["boundary_index"] = 1
    else:
        function["ops"][0]["kind"] = "ret_void"
    with pytest.raises(ValueError):
        project_frontend_tir_to_executable(document)
