from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_stdlib_policy

_MODULE_STDLIB_POLICY_NAMES = (
    "_CORE_STDLIB_MODULES_FULL",
    "_CORE_STDLIB_MODULES_MICRO",
    "_INTRINSIC_CALL_NAMES",
    "_STDLIB_POLICY_GATE_STATUS",
    "_STDLIB_PROBE_INTRINSIC",
    "_build_stdlib_like_module_flags",
    "_core_stdlib_module_names_for_profile",
    "_enforce_intrinsic_stdlib",
    "_enforce_profile_feature_availability",
    "_ensure_core_stdlib_modules",
    "_is_fail_closed_import_policy_gate",
    "_looks_like_stdlib_module_name",
    "_module_required_intrinsic_names",
    "_profile_feature_gap_for_module",
    "_stdlib_allowlist",
    "_stdlib_allowlist_cached",
    "_stdlib_module_intrinsic_status",
)

_MODULE_STDLIB_POLICY_DEFINITIONS = (
    "_CORE_STDLIB_MODULES_FULL = (",
    "_CORE_STDLIB_MODULES_MICRO = (",
    "_INTRINSIC_CALL_NAMES = {",
    "_STDLIB_POLICY_GATE_STATUS =",
    "_STDLIB_PROBE_INTRINSIC =",
    "def _build_stdlib_like_module_flags(",
    "def _core_stdlib_module_names_for_profile(",
    "def _enforce_intrinsic_stdlib(",
    "def _enforce_profile_feature_availability(",
    "def _ensure_core_stdlib_modules(",
    "def _is_fail_closed_import_policy_gate(",
    "def _looks_like_stdlib_module_name(",
    "def _module_required_intrinsic_names(",
    "def _profile_feature_gap_for_module(",
    "def _stdlib_allowlist(",
    "def _stdlib_allowlist_cached(",
    "def _stdlib_module_intrinsic_status(",
)


def test_cli_module_stdlib_policy_authority_is_single_home() -> None:
    for name in _MODULE_STDLIB_POLICY_NAMES:
        assert hasattr(module_stdlib_policy, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_STDLIB_POLICY_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source
