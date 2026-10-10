from __future__ import annotations

import inspect
from pathlib import Path

import molt.cli as cli
from molt import stdlib_intrinsic_policy
from molt.cli import module_graph
from molt.cli import module_stdlib_policy

_MODULE_STDLIB_POLICY_NAMES = (
    "_CORE_STDLIB_MODULES_FULL",
    "_CORE_STDLIB_MODULES_MICRO",
    "_build_stdlib_like_module_flags",
    "_core_stdlib_module_names_for_profile",
    "_ensure_core_stdlib_modules",
    "_looks_like_stdlib_module_name",
    "_stdlib_allowlist",
    "_stdlib_allowlist_cached",
)

_MODULE_STDLIB_POLICY_DEFINITIONS = (
    "_CORE_STDLIB_MODULES_FULL = (",
    "_CORE_STDLIB_MODULES_MICRO = (",
    "def _build_stdlib_like_module_flags(",
    "def _core_stdlib_module_names_for_profile(",
    "def _ensure_core_stdlib_modules(",
    "def _looks_like_stdlib_module_name(",
    "def _stdlib_allowlist(",
    "def _stdlib_allowlist_cached(",
)

_SHARED_STDLIB_INTRINSIC_POLICY_DEFINITIONS = (
    "INTRINSIC_CALL_NAMES = frozenset(",
    "STATUS_INTRINSIC_SUPPORT =",
    "STATUS_PYTHON_COMPILED =",
    "STATUS_STUB =",
    "def classify_stdlib_module_statuses(",
    "def intrinsic_names_from_source(",
    "def is_fail_closed_import_policy_gate(",
    "def module_required_intrinsic_names(",
    "def stdlib_module_intrinsic_facts(",
    "def stdlib_module_static_imports(",
    "def stdlib_module_import_evidence(",
)

_LEGACY_LOCAL_STDLIB_INTRINSIC_POLICY_DEFINITIONS = (
    "\n_INTRINSIC_CALL_NAMES = {",
    '_STDLIB_POLICY_GATE_STATUS = "policy-gate"',
    '_STDLIB_PROBE_INTRINSIC = "molt_stdlib_probe"',
    # Builds admit compiled pure-Python stdlib modules (spec 0016), so the
    # build-time python-only rejection must not return.
    "def _enforce_intrinsic_stdlib(",
    "def _enforce_profile_feature_availability(",
    "def _is_fail_closed_import_policy_gate(",
    "def _module_relative_import_base(",
    "def _module_required_intrinsic_names(",
    "def _profile_feature_gap_for_module(",
    "def _same_package_intrinsic_import_closure(",
    "def _stdlib_module_intrinsic_status(",
    "def _stdlib_module_static_imports(",
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


def test_stdlib_intrinsic_classifier_is_shared_with_audit_tool() -> None:
    shared_source = inspect.getsource(stdlib_intrinsic_policy)
    module_policy_source = inspect.getsource(module_stdlib_policy)
    audit_tool_source = (
        Path(__file__).resolve().parents[2] / "tools" / "check_stdlib_intrinsics.py"
    ).read_text(encoding="utf-8")

    for marker in _SHARED_STDLIB_INTRINSIC_POLICY_DEFINITIONS:
        assert marker in shared_source
    for marker in _LEGACY_LOCAL_STDLIB_INTRINSIC_POLICY_DEFINITIONS:
        assert marker not in module_policy_source
        assert marker not in audit_tool_source
    assert "from molt.stdlib_intrinsic_policy import" in audit_tool_source
