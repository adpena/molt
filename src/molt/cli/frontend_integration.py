from __future__ import annotations

from collections.abc import Collection, Mapping
from typing import Any

from molt.frontend import SimpleTIRGenerator
from molt.frontend import ClassId, ClassInfo, layout_definition_symbol
from molt.cli.models import _FrontendIntegrationState, _MidendDiagnosticsState


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


def _integrate_module_code_slots_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    funcs: list[dict[str, Any]],
    local_symbols: set[str],
    class_definitions: Collection[str],
) -> list[dict[str, Any]]:
    """Project module code owners without changing the borrowed lowering result."""
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    integrated_functions: list[dict[str, Any]] = []
    for func in funcs:
        source_ops = func.get("ops", [])
        integrated_ops: list[dict[str, Any]] | None = None
        for index, original_op in enumerate(source_ops):
            op = original_op
            kind = op.get("kind")
            if kind == "code_slots_init":
                if integrated_ops is None:
                    integrated_ops = source_ops[:index]
                continue
            if kind in {"code_slot_set", "trace_enter_slot"}:
                symbol = op.get("s_value")
                if symbol not in local_symbols or op.get("value") is not None:
                    raise ValueError(
                        f"Missing canonical code owner {symbol!r} in module {module_name}"
                    )
                if symbol == "molt_main":
                    symbol = init_symbol
                    op = {**op, "s_value": symbol}
                _register_global_code_id_with_state(integration_state, symbol)
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
                integration_state.class_definition_references.setdefault(
                    definition, module_name
                )
                _register_global_code_id_with_state(integration_state, definition)
            if integrated_ops is None and op is not original_op:
                integrated_ops = source_ops[:index]
            if integrated_ops is not None:
                integrated_ops.append(op)
        if integrated_ops is not None or func.get("name") == "molt_main":
            projected = dict(func)
            if integrated_ops is not None:
                projected["ops"] = integrated_ops
            if func.get("name") == "molt_main":
                projected["name"] = init_symbol
            integrated_functions.append(projected)
        else:
            integrated_functions.append(func)
    return integrated_functions


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
    func_code_ids: dict[str, int],
    local_classes: dict[ClassId, ClassInfo],
) -> str | None:
    class_definitions = _module_class_definitions(local_classes)
    for symbol in sorted(class_definitions):
        owner = integration_state.class_definition_owners.setdefault(
            symbol, module_name
        )
        if owner != module_name:
            return (
                f"Class statement {symbol!r} is defined by both module {owner} "
                f"and module {module_name}"
            )
    try:
        integrated_functions = _integrate_module_code_slots_with_state(
            integration_state,
            module_name,
            ir_functions,
            set(func_code_ids),
            class_definitions,
        )
    except ValueError as exc:
        return str(exc)
    integration_state.functions.extend(integrated_functions)
    # ClassIds are program-wide identities: one module's classes never
    # replace another module's same-named classes.
    integration_state.known_classes.update(local_classes)
    return None
