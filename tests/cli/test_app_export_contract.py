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
        entry_module="probe",
        known_modules={"probe", "helpers"},
        known_func_kinds=known_kinds,
    )
    generator.visit(tree)
    ir = generator.to_json()
    return ir, build_app_export_contract(
        entry_module="probe",
        ir=ir,
        registry_digest="a" * 64,
    )


def test_app_export_contract_owns_binding_families_and_last_definition() -> None:
    ir, contract = _frontend_ir_and_contract(
        """
from helpers import imported

def _identity(fn):
    return fn

def alpha(value):
    return value

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

alias = alpha

def gone(value):
    return value

del gone
"""
    )

    carrier = next(
        function["app_callable_bindings"]
        for function in ir["functions"]
        if "app_callable_bindings" in function
    )
    assert carrier == contract["bindings"]
    bindings = {binding["name"]: binding for binding in contract["bindings"]}
    assert exported_app_symbols(contract) == (
        "probe__alpha",
        "probe__beta",
        "probe__same",
    )
    assert bindings["same"]["superseded_symbols"] == []
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
            "ownership": "borrowed",
        },
        "result": {
            "representation": "tagged-i64",
            "ownership": "owned",
        },
        "adapter": {
            "strategy": "retain-result",
            "symbol_prefix": "__molt_export_alias__",
            "retain_import": {
                "module": "molt_runtime",
                "name": "molt_inc_ref_obj",
                "parameters": ["i64"],
                "results": [],
            },
        },
    }

    contract["call_abi"]["result"]["ownership"] = "borrowed"
    with pytest.raises(ValueError, match="canonical molt.wasm.app-call"):
        validate_app_export_contract(contract)


def test_contract_builder_only_projects_frontend_resolved_metadata() -> None:
    parameters = inspect.signature(build_app_export_contract).parameters
    assert tuple(parameters) == ("entry_module", "ir", "registry_digest")
    source = inspect.getsource(app_export_contract)
    assert "import ast" not in source
    assert "NodeVisitor" not in source
    with pytest.raises(ValueError, match="frontend-resolved"):
        build_app_export_contract(
            entry_module="probe",
            ir={"functions": []},
            registry_digest="a" * 64,
        )
