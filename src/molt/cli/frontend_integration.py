from __future__ import annotations

from collections.abc import Collection, Mapping
from typing import Any

from molt.frontend import SimpleTIRGenerator
from molt.frontend import ClassId, ClassInfo, layout_definition_symbol
from molt.cli.models import _FrontendIntegrationState, _MidendDiagnosticsState
from molt.frontend.module_publication import (
    inspect_source_module_publication,
    parse_function_metadata,
    reject_embedded_function_metadata,
    reject_embedded_operation_metadata,
)


def _register_global_code_id_with_state(
    integration_state: _FrontendIntegrationState,
    symbol: str,
) -> int:
    code_id = integration_state.global_code_ids.get(symbol)
    if code_id is None:
        code_id = len(integration_state.global_code_ids)
        integration_state.global_code_ids[symbol] = code_id
    return code_id


def _module_class_definitions(local_classes: Mapping[ClassId, ClassInfo]) -> set[str]:
    """The class statement symbols a module owns: the ones it stamps (HF-220)."""
    return {
        symbol
        for class_id, info in local_classes.items()
        if (symbol := layout_definition_symbol(class_id, info)) is not None
    }


def _plan_module_code_slots(
    module_name: str,
    funcs: list[dict[str, Any]],
    local_symbols: set[str],
    class_definitions: Collection[str],
) -> tuple[dict[str, dict[str, Any]], dict[str, None], dict[str, str]]:
    """Validate one module and stage its ordered additions without mutation."""
    if "molt_main" in local_symbols:
        raise ValueError(
            f"Noncanonical module code owner 'molt_main' in module {module_name}"
        )
    owned_functions: dict[str, dict[str, Any]] = {}
    code_symbols: dict[str, None] = {}
    class_references: dict[str, str] = {}
    for func in funcs:
        reject_embedded_function_metadata(func)
        name = func.get("name")
        if not isinstance(name, str) or name in owned_functions:
            raise ValueError(f"Invalid or duplicate executable function owner {name!r}")
        owned_functions[name] = func
        if func.get("name") == "molt_main":
            raise ValueError(
                f"Legacy program entry 'molt_main' in module {module_name}"
            )
        for op in func.get("ops", []):
            reject_embedded_operation_metadata(op)
            kind = op.get("kind")
            if kind == "code_slots_init":
                raise ValueError(
                    f"Module {module_name} cannot initialize program code slots"
                )
            if kind in {"code_slot_set", "trace_enter_slot"}:
                symbol = op.get("s_value")
                if symbol not in local_symbols or op.get("value") is not None:
                    raise ValueError(
                        f"Missing canonical code owner {symbol!r} in module {module_name}"
                    )
                code_symbols.setdefault(symbol, None)
            definition = op.get("class_definition")
            if definition is not None:
                # A stamp names its own module's statement; a layout guard may
                # name any module's statement (an imported class), so its owner
                # is checked once the whole program is integrated.
                if (
                    kind == "class_set_definition"
                    and definition not in class_definitions
                ):
                    raise ValueError(
                        f"Class statement {definition!r} is stamped outside its "
                        f"owning module {module_name}"
                    )
                class_references.setdefault(definition, module_name)
                code_symbols.setdefault(definition, None)
    return owned_functions, code_symbols, class_references


def _validate_class_definition_references_with_state(
    integration_state: _FrontendIntegrationState,
) -> str | None:
    """Every class statement a stamp or guard names has exactly one owner."""
    for symbol, module_name in sorted(
        integration_state.class_definition_references.items()
    ):
        if symbol not in integration_state.class_definition_owners:
            return (
                f"Class statement {symbol!r} named by module {module_name} is "
                "defined by no module of the program"
            )
    return None


def _accumulate_midend_diagnostics_with_state(
    diagnostics_state: _MidendDiagnosticsState,
    module_name: str,
    *,
    policy_outcomes_by_func: dict[str, dict[str, Any]],
    pass_stats_by_func: dict[str, dict[str, dict[str, Any]]],
) -> None:
    def normalize_function_name(function_name: str) -> str:
        if function_name == "molt_main":
            return SimpleTIRGenerator.module_init_symbol(module_name)
        return function_name

    for function_name in sorted(policy_outcomes_by_func):
        normalized_name = normalize_function_name(function_name)
        combined_name = f"{module_name}::{normalized_name}"
        outcome = policy_outcomes_by_func[function_name]
        copied_events: list[dict[str, Any]] = []
        for event in outcome.get("degrade_events", []):
            if isinstance(event, dict):
                copied_events.append(dict(event))
        copied_outcome = dict(outcome)
        copied_outcome["degrade_events"] = copied_events
        diagnostics_state.policy_outcomes_by_function[combined_name] = copied_outcome
    for function_name in sorted(pass_stats_by_func):
        normalized_name = normalize_function_name(function_name)
        combined_name = f"{module_name}::{normalized_name}"
        per_pass = pass_stats_by_func[function_name]
        copied_per_pass: dict[str, dict[str, Any]] = {}
        for pass_name in sorted(per_pass):
            copied_stats = dict(per_pass[pass_name])
            samples = copied_stats.get("samples_ms")
            if isinstance(samples, list):
                copied_stats["samples_ms"] = list(samples)
            copied_per_pass[pass_name] = copied_stats
        diagnostics_state.pass_stats_by_function[combined_name] = copied_per_pass


def _integrate_module_frontend_result_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    *,
    ir_functions: list[dict[str, Any]],
    function_metadata: object,
    func_code_ids: dict[str, int],
    local_classes: dict[ClassId, ClassInfo],
) -> str | None:
    class_definitions = _module_class_definitions(local_classes)
    for symbol in sorted(class_definitions):
        owner = integration_state.class_definition_owners.get(symbol)
        if owner is not None and owner != module_name:
            return (
                f"Class statement {symbol!r} is defined by both module {owner} "
                f"and module {module_name}"
            )
    try:
        owned_functions, code_symbols, class_references = _plan_module_code_slots(
            module_name,
            ir_functions,
            set(func_code_ids),
            class_definitions,
        )
        metadata = parse_function_metadata(
            function_metadata, function_symbols=owned_functions.keys()
        )
        for symbol, row in metadata.items():
            if symbol in integration_state.function_metadata:
                raise ValueError(f"Duplicate function metadata owner {symbol!r}")
            inspect_source_module_publication(owned_functions[symbol], row)
    except ValueError as exc:
        return str(exc)
    # Admission succeeded. Commit only this module's staged additions; copying
    # accumulated program state here would grow quadratically with modules.
    for symbol in code_symbols:
        _register_global_code_id_with_state(integration_state, symbol)
    for symbol in sorted(class_definitions):
        integration_state.class_definition_owners.setdefault(symbol, module_name)
    for symbol, owner in class_references.items():
        integration_state.class_definition_references.setdefault(symbol, owner)
    integration_state.functions.extend(ir_functions)
    integration_state.function_metadata.update(metadata)
    # ClassIds are program-wide identities: one module's classes never
    # replace another module's same-named classes.
    integration_state.known_classes.update(local_classes)
    return None
