from __future__ import annotations

import ast
import inspect

import pytest

import molt.cli.app_export_contract as app_export_contract

from molt.cli.app_export_contract import (
    APP_EXPORT_CALL_ABI_SCHEMA,
    APP_EXPORT_CONTRACT_SCHEMA,
    app_export_call_abi,
    build_app_export_contract,
    excluded_app_symbols,
    exported_app_symbols,
    validate_app_export_contract,
)
from molt.frontend import SimpleTIRGenerator
from molt.frontend._types import _MOLT_CLOSURE_PARAM
from molt.frontend.sema import collect_module_func_kinds


def _frontend_ir_and_contract(
    source: str,
) -> tuple[dict[str, object], dict[str, object]]:
    tree = ast.parse(source)
    known_kinds = {
        "probe": {
            name: kind.value for name, kind in collect_module_func_kinds(tree).items()
        },
        "helpers": {"imported": "sync"},
    }
    generator = SimpleTIRGenerator(
        module_name="probe",
        module_execution_kind="module",
        known_modules={"probe", "helpers"},
        known_func_kinds=known_kinds,
        # ``helpers`` is compiled into the same build, so its declared
        # functions have linkable code symbols.
        direct_call_modules={"helpers"},
    )
    generator.visit(tree)
    ir = generator.to_json(scope="module")
    return ir, build_app_export_contract(
        entry_module="probe",
        ir=ir,
        function_metadata=ir["function_metadata"],
        registry_digest="a" * 64,
    )


def test_app_export_contract_owns_binding_families_and_last_definition() -> None:
    ir, contract = _frontend_ir_and_contract(
        """
def _identity(fn):
    return fn

def alpha(value):
    return value

# Stated before any other code runs: an import or a call such as the
# decorator below may rebind any global, so a later alias has no provable
# callable identity.
alias = alpha

from helpers import imported

def beta(left, right):
    return left

def replaced(value):
    return value

replaced = 7

def same(value):
    return value

def same(value):
    return value

def _private(value):
    return value

@_identity
def decorated(value):
    return value

def gone(value):
    return value

del gone
"""
    )

    carrier = next(
        metadata["app_callable_bindings"]
        for metadata in ir["function_metadata"].values()
        if "app_callable_bindings" in metadata
    )
    assert carrier == contract["bindings"]
    bindings = {binding["name"]: binding for binding in contract["bindings"]}
    # A redefined name exports its final definition and records the one it
    # replaced, so the earlier symbol can never be dispatched by name.
    assert exported_app_symbols(contract) == (
        "probe__alpha",
        "probe__beta",
        "probe__same_1",
    )
    assert bindings["same"]["superseded_symbols"] == ["probe__same"]
    assert bindings["_private"]["reason"] == "private-name"
    assert bindings["decorated"]["reason"] == (
        "decorated-binding-requires-module-dispatch"
    )
    assert bindings["imported"]["reason"] == (
        "imported-binding-requires-module-dispatch"
    )
    assert bindings["replaced"]["reason"] == "dynamic-rebound-binding"
    assert bindings["alias"]["reason"] == (
        "dynamic-callable-alias-requires-module-dispatch"
    )
    assert bindings["gone"]["reason"] == "deleted-binding"
    assert set(excluded_app_symbols(contract)) >= {
        "probe___identity",
        "probe___private",
        "probe__decorated",
        "probe__replaced",
    }


def test_app_export_contract_digest_rejects_mutation() -> None:
    _ir, contract = _frontend_ir_and_contract("def alpha(value):\n    return value\n")
    contract["registry_digest"] = "b" * 64
    with pytest.raises(ValueError, match="digest mismatch"):
        validate_app_export_contract(contract)


def test_app_export_contract_seals_owned_result_boundary() -> None:
    _ir, contract = _frontend_ir_and_contract("def alpha(value):\n    return value\n")

    assert contract["schema"] == APP_EXPORT_CONTRACT_SCHEMA
    call_abi = app_export_call_abi(contract)
    assert call_abi == {
        "schema": APP_EXPORT_CALL_ABI_SCHEMA,
        "name": "molt.wasm.app-call",
        "parameters": {
            "representation": "tagged-i64",
            "ownership": "transferred",
        },
        "result": {
            "representation": "tagged-i64",
            "ownership": "owned",
        },
        "adapter": {
            "strategy": "forward-owned-result",
            "symbol_prefix": "__molt_export_alias__",
        },
    }

    contract["call_abi"]["result"]["ownership"] = "borrowed"
    with pytest.raises(ValueError, match="canonical molt.wasm.app-call"):
        validate_app_export_contract(contract)


def test_app_export_contract_requires_entries_that_adopt_their_arguments() -> None:
    source = "def alpha(value, other):\n    return value\n\ndef zero():\n    return 1\n"
    ir, contract = _frontend_ir_and_contract(source)
    functions = ir["functions"]
    assert isinstance(functions, list)
    entries = {function["name"]: function for function in functions}
    # A synchronous frame's entry adopts every argument; a zero-parameter
    # entry declares nothing. Both satisfy transferred-parameter app calls.
    assert entries["probe__alpha"]["parameter_custody"] == [
        "transferred",
        "transferred",
    ]
    assert "parameter_custody" not in entries["probe__zero"]
    assert exported_app_symbols(contract) == ("probe__alpha", "probe__zero")

    # A borrowing entry under the transferred-parameter ABI would have every
    # host release each argument a second time.
    entries["probe__alpha"]["parameter_custody"] = ["transferred", "borrowed"]
    with pytest.raises(ValueError, match="must adopt every parameter"):
        build_app_export_contract(
            entry_module="probe",
            ir=ir,
            function_metadata=ir["function_metadata"],
            registry_digest="a" * 64,
        )
    del entries["probe__alpha"]["parameter_custody"]
    with pytest.raises(ValueError, match="must adopt every parameter"):
        build_app_export_contract(
            entry_module="probe",
            ir=ir,
            function_metadata=ir["function_metadata"],
            registry_digest="a" * 64,
        )


def test_contract_builder_only_projects_frontend_resolved_metadata() -> None:
    parameters = inspect.signature(build_app_export_contract).parameters
    assert set(parameters) == {
        "entry_module",
        "ir",
        "function_metadata",
        "registry_digest",
    }
    assert parameters["function_metadata"].default is inspect.Parameter.empty
    source = inspect.getsource(app_export_contract)
    assert "import ast" not in source
    assert "NodeVisitor" not in source
    with pytest.raises(ValueError, match="frontend-resolved"):
        build_app_export_contract(
            entry_module="probe",
            ir={"functions": []},
            function_metadata={},
            registry_digest="a" * 64,
        )


@pytest.mark.parametrize("defect", ["missing", "orphan", "duplicate-table", "embedded"])
def test_app_contract_requires_one_owned_sidecar_table(defect: str) -> None:
    from copy import deepcopy

    ir, _ = _frontend_ir_and_contract("def alpha(value):\n    return value\n")
    metadata = ir["function_metadata"]
    owner = next(
        name for name, row in metadata.items() if "app_callable_bindings" in row
    )
    if defect == "missing":
        del metadata[owner]["app_callable_bindings"]
    elif defect == "orphan":
        metadata["absent"] = metadata.pop(owner)
    elif defect == "duplicate-table":
        metadata["probe__alpha"] = {"app_callable_bindings": []}
    else:
        ir["functions"][0]["app_callable_bindings"] = []
    before = deepcopy(ir)
    with pytest.raises(ValueError):
        build_app_export_contract(
            entry_module="probe",
            ir=ir,
            function_metadata=metadata,
            registry_digest="a" * 64,
        )
    assert ir == before


def test_app_exports_use_body_capture_not_generic_syntax():
    ir, contract = _frontend_ir_and_contract(
        "def captured[T](value): return T\n"
        "def closure_only[T](): return T\n"
        "def uncaptured[T](value): return value\n"
        "def annotations_only[T](value: T) -> T: return value\n"
    )
    bindings = {row["name"]: row for row in contract["bindings"]}
    entries = {row["name"]: row for row in ir["functions"]}
    assert exported_app_symbols(contract) == (
        "probe__annotations_only",
        "probe__uncaptured",
    )
    for name in ("captured", "closure_only"):
        row = bindings[name]
        assert row["disposition"] == "excluded"
        assert row["reason"] == "closure-callable-requires-module-dispatch"
        assert row["symbol"] in excluded_app_symbols(contract)
        assert entries[row["symbol"]]["params"][0] == _MOLT_CLOSURE_PARAM
    assert entries["probe__captured"]["parameter_custody"] == [
        "borrowed",
        "transferred",
    ]
    assert "parameter_custody" not in entries["probe__closure_only"]
    for name in ("uncaptured", "annotations_only"):
        assert _MOLT_CLOSURE_PARAM not in entries[bindings[name]["symbol"]]["params"]
        assert entries[bindings[name]["symbol"]]["parameter_custody"] == ["transferred"]


def test_forged_flat_export_of_closure_keeps_custody_refusal():
    ir, _contract = _frontend_ir_and_contract("def captured[T](value): return T\n")
    carrier = next(
        row["app_callable_bindings"]
        for row in ir["function_metadata"].values()
        if "app_callable_bindings" in row
    )
    binding = next(row for row in carrier if row["name"] == "captured")
    binding["disposition"] = "export"
    binding["reason"] = None
    with pytest.raises(ValueError, match="must adopt every parameter"):
        build_app_export_contract(
            entry_module="probe",
            ir=ir,
            function_metadata=ir["function_metadata"],
            registry_digest="a" * 64,
        )


@pytest.mark.parametrize(
    ("prefix", "body", "kind", "reason"),
    [
        ("", "return T", "sync", "closure-callable-requires-module-dispatch"),
        ("", "yield T", "gen", "stateful-callable-requires-module-dispatch"),
        ("async ", "return T", "async", "stateful-callable-requires-module-dispatch"),
        ("async ", "yield T", "asyncgen", "stateful-callable-requires-module-dispatch"),
    ],
)
def test_each_source_callable_producer_preserves_context_exclusion(
    prefix, body, kind, reason
):
    _ir, contract = _frontend_ir_and_contract(f"{prefix}def captured[T](): {body}\n")
    (binding,) = contract["bindings"]
    assert binding["kind"] == kind
    assert binding["disposition"] == "excluded"
    assert binding["reason"] == reason
    assert exported_app_symbols(contract) == ()
    assert binding["symbol"] in excluded_app_symbols(contract)


@pytest.mark.parametrize("final_capture", [False, True])
def test_closure_export_policy_follows_final_definition(final_capture):
    closed = "def item[T](value): return T\n"
    plain = "def item(value): return value\n"
    _ir, contract = _frontend_ir_and_contract(
        plain + closed if final_capture else closed + plain
    )
    (binding,) = contract["bindings"]
    assert binding["symbol"] == "probe__item_1"
    assert binding["superseded_symbols"] == ["probe__item"]
    assert "probe__item" in excluded_app_symbols(contract)
    assert exported_app_symbols(contract) == (
        () if final_capture else ("probe__item_1",)
    )
    if final_capture:
        assert binding["reason"] == "closure-callable-requires-module-dispatch"
