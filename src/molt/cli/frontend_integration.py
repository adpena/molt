from __future__ import annotations

from typing import Any

from molt.frontend import SimpleTIRGenerator
from molt.cli.models import _FrontendIntegrationState, _MidendDiagnosticsState


def _register_global_code_id_with_state(
    integration_state: _FrontendIntegrationState,
    symbol: str,
) -> int:
    code_id = integration_state.global_code_ids.get(symbol)
    if code_id is None:
        code_id = integration_state.global_code_id_counter
        integration_state.global_code_ids[symbol] = code_id
        integration_state.global_code_id_counter += 1
    return code_id


def _remap_module_code_ops_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    funcs: list[dict[str, Any]],
    local_id_to_symbol: dict[int, str],
) -> None:
    for func in funcs:
        ops = func.get("ops", [])
        remapped_ops: list[dict[str, Any]] = []
        for op in ops:
            kind = op.get("kind")
            if kind == "code_slots_init":
                continue
            if kind in {"call", "call_internal"}:
                symbol = op.get("s_value")
                if symbol:
                    op["value"] = _register_global_code_id_with_state(
                        integration_state, symbol
                    )
            elif kind == "code_slot_set":
                local_id = op.get("value")
                symbol = local_id_to_symbol.get(local_id)
                if symbol is None:
                    raise ValueError(
                        f"Missing code symbol for id {local_id} in module {module_name}"
                    )
                op["value"] = _register_global_code_id_with_state(
                    integration_state, symbol
                )
            elif kind == "trace_enter_slot":
                local_id = op.get("value")
                symbol = local_id_to_symbol.get(local_id)
                if symbol is None:
                    raise ValueError(
                        f"Missing code symbol for id {local_id} in module {module_name}"
                    )
                op["value"] = _register_global_code_id_with_state(
                    integration_state, symbol
                )
            remapped_ops.append(op)
        func["ops"] = remapped_ops


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
    local_class_names: list[str],
    local_classes: dict[str, Any],
) -> str | None:
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    local_code_ids = dict(func_code_ids)
    if "molt_main" in local_code_ids:
        local_code_ids[init_symbol] = local_code_ids.pop("molt_main")
    local_id_to_symbol = {code_id: symbol for symbol, code_id in local_code_ids.items()}
    try:
        _remap_module_code_ops_with_state(
            integration_state,
            module_name,
            ir_functions,
            local_id_to_symbol,
        )
    except ValueError as exc:
        return str(exc)
    for func in ir_functions:
        if func["name"] == "molt_main":
            func["name"] = init_symbol
    integration_state.functions.extend(ir_functions)
    for class_name in local_class_names:
        class_info = local_classes.get(class_name)
        if class_info is not None:
            integration_state.known_classes[class_name] = class_info
    return None
