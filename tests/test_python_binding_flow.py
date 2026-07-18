from __future__ import annotations

import ast
from concurrent.futures import ThreadPoolExecutor

import pytest

from molt.compiler_analysis.python_binding_facts import (
    OTHER_IDENTITY,
    PythonIdentity,
    PythonMember,
    identity_fact_is_exact,
    identity_fact_may_be,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_source_bindings,
)
from molt.compiler_analysis.python_effects_generated import (
    PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS,
    effect_mask_satisfies_capability,
)


def _last_call(source: str):
    index = analyze_python_source_bindings(source)
    assert index.calls
    return index.calls[-1]


def _identity_on_line(source: str, line: int, identity: PythonIdentity) -> bool:
    index = analyze_python_source_bindings(source)
    return any(
        fact.node.lineno == line
        and identity_fact_is_exact(fact.identities, identity)
        for fact in index.expressions
    )


def test_alias_chain_preserves_exact_import_module_identity() -> None:
    call = _last_call(
        "import importlib as loader\n"
        "load = loader.import_module\n"
        "load('pkg.leaf')\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert not call.callee_identities & OTHER_IDENTITY


@pytest.mark.parametrize(
    "source",
    [
        "import importlib.util as util\nutil.find_spec('pkg.leaf')\n",
        "import importlib\nimportlib.util.find_spec('pkg.leaf')\n",
        "from importlib.util import find_spec as find\nfind('pkg.leaf')\n",
    ],
)
def test_find_spec_forms_share_exact_identity(source: str) -> None:
    call = _last_call(source)
    assert call.callee_is(PythonIdentity.IMPORTLIB_FIND_SPEC)


def test_find_spec_member_rebinding_invalidates_all_aliases() -> None:
    call = _last_call(
        "import importlib.util as util\n"
        "alias = util\n"
        "alias.find_spec = replacement\n"
        "util.find_spec('pkg.leaf')\n"
    )
    assert call.callee_identities == OTHER_IDENTITY
    assert call.definitely_invalidated_members_after & int(
        PythonMember.UTIL_FIND_SPEC
    )


@pytest.mark.parametrize(
    "conditional",
    [
        "if flag:\n    importlib = replacement\n",
        "for item in items:\n    importlib = replacement\n",
        "while flag:\n    importlib = replacement\n",
        "try:\n    importlib = replacement\nexcept Exception:\n    pass\n",
        "match token:\n    case 1:\n        importlib = replacement\n",
    ],
)
def test_control_flow_join_retains_both_canonical_and_shadowed_identity(
    conditional: str,
) -> None:
    call = _last_call(
        "import importlib\n"
        f"{conditional}"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    assert not call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


def test_statically_dead_branch_does_not_manufacture_possible_shadow() -> None:
    call = _last_call(
        "import importlib\n"
        "if False:\n"
        "    importlib = replacement\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


@pytest.mark.parametrize(
    "expression",
    [
        "False and importlib.import_module('pkg.dead')",
        "True or importlib.import_module('pkg.dead')",
    ],
)
def test_boolean_short_circuit_does_not_index_dead_call(expression: str) -> None:
    index = analyze_python_source_bindings(f"import importlib\n{expression}\n")
    assert index.calls == ()


def test_unconditional_member_rebinding_removes_canonical_identity() -> None:
    call = _last_call(
        "import importlib\n"
        "importlib.import_module = replacement\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_identities == OTHER_IDENTITY
    assert call.definitely_invalidated_members_after & int(
        PythonMember.IMPORTLIB_IMPORT_MODULE
    )


def test_conditional_member_rebinding_is_possible_not_definite() -> None:
    call = _last_call(
        "import importlib\n"
        "if flag:\n"
        "    importlib.import_module = replacement\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    assert call.maybe_invalidated_members_after & int(
        PythonMember.IMPORTLIB_IMPORT_MODULE
    )
    assert not call.definitely_invalidated_members_after & int(
        PythonMember.IMPORTLIB_IMPORT_MODULE
    )


def test_function_parameter_shadows_outer_importlib_for_whole_scope() -> None:
    call = _last_call(
        "import importlib\n"
        "def load(importlib):\n"
        "    return importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_identities == OTHER_IDENTITY


def test_deferred_function_excludes_impossible_pre_definition_module_state() -> None:
    call = _last_call(
        "import importlib\n"
        "def load():\n"
        "    return importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


def test_nested_closure_preserves_exact_outer_alias() -> None:
    call = _last_call(
        "def outer():\n"
        "    import importlib\n"
        "    def load():\n"
        "        return importlib.import_module('pkg.leaf')\n"
        "    return load\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)


def test_global_binding_obeys_source_order_inside_function() -> None:
    call = _last_call(
        "import importlib\n"
        "def load(flag):\n"
        "    global importlib\n"
        "    if flag:\n"
        "        importlib = replacement\n"
        "    return importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_nonlocal_binding_obeys_source_order_inside_nested_function() -> None:
    call = _last_call(
        "def outer():\n"
        "    import importlib\n"
        "    def load(flag):\n"
        "        nonlocal importlib\n"
        "        if flag:\n"
        "            importlib = replacement\n"
        "        return importlib.import_module('pkg.leaf')\n"
        "    return load\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_module_spec_alias_and_constructor_result_are_tracked() -> None:
    call = _last_call(
        "from importlib.machinery import ModuleSpec as Spec\n"
        "Alias = Spec\n"
        "value = Alias('pkg.leaf', None)\n"
    )
    assert call.callee_is(PythonIdentity.MODULE_SPEC_CLASS)
    assert identity_fact_is_exact(
        call.result_identities, PythonIdentity.MODULE_SPEC_INSTANCE
    )


def test_module_spec_member_mutation_invalidates_all_aliases() -> None:
    call = _last_call(
        "import importlib.machinery as machinery\n"
        "alias = machinery\n"
        "alias.ModuleSpec = replacement\n"
        "value = machinery.ModuleSpec('pkg.leaf', None)\n"
    )
    assert call.callee_identities == OTHER_IDENTITY
    assert call.definitely_invalidated_members_after & int(
        PythonMember.MACHINERY_MODULE_SPEC
    )


def test_globals_module_and_frame_capabilities_share_exact_identities() -> None:
    source = (
        "import sys\n"
        "import inspect\n"
        "global_map = globals()\n"
        "module = sys.modules[__name__]\n"
        "frame = inspect.currentframe()\n"
        "frame_globals = frame.f_globals\n"
        "def local():\n"
        "    pass\n"
        "function_globals = local.__globals__\n"
    )
    assert _identity_on_line(source, 3, PythonIdentity.CURRENT_GLOBALS)
    assert _identity_on_line(source, 4, PythonIdentity.CURRENT_MODULE)
    assert _identity_on_line(source, 5, PythonIdentity.CURRENT_FRAME)
    assert _identity_on_line(source, 6, PythonIdentity.CURRENT_GLOBALS)
    assert _identity_on_line(source, 9, PythonIdentity.CURRENT_GLOBALS)


def test_setattr_and_exec_invalidate_import_identity_without_losing_possibility() -> None:
    setattr_call = _last_call(
        "import importlib\n"
        "setattr(importlib, 'import_module', replacement)\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert setattr_call.callee_identities == OTHER_IDENTITY

    exec_call = _last_call(
        "import importlib\n"
        "exec(source)\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert exec_call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert exec_call.callee_identities & OTHER_IDENTITY


@pytest.mark.parametrize(
    "mutation",
    [
        "globals()['importlib'] = replacement\n",
        "vars()['importlib'] = replacement\n",
        (
            "import inspect\n"
            "inspect.currentframe().f_globals['importlib'] = replacement\n"
        ),
    ],
)
def test_global_and_frame_reflection_taints_exposed_bindings(mutation: str) -> None:
    call = _last_call(
        "import importlib\n"
        f"{mutation}"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_builtins_import_alias_and_member_mutation_share_one_authority() -> None:
    exact = _last_call(
        "from builtins import __import__ as load\n"
        "load('pkg.leaf')\n"
    )
    assert exact.callee_is(PythonIdentity.BUILTINS_IMPORT)

    mutated = _last_call(
        "import builtins\n"
        "builtins.__import__ = replacement\n"
        "__import__('pkg.leaf')\n"
    )
    assert mutated.callee_identities == OTHER_IDENTITY


def test_import_hook_mutation_downgrades_later_standard_import_to_possible() -> None:
    call = _last_call(
        "import sys\n"
        "sys.meta_path = hooks\n"
        "import importlib\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY
    assert call.maybe_invalidated_members_after & int(PythonMember.IMPORT_HOOKS)


def test_reference_release_callbacks_poison_exposed_global_bindings() -> None:
    call = _last_call(
        "import importlib\n"
        "owned = arbitrary\n"
        "owned = 1\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_import_calls_fail_preserves_import_state_capability() -> None:
    call = _last_call(
        "from importlib import import_module\n"
        "import_module('pkg.leaf')\n"
    )
    assert not effect_mask_satisfies_capability(
        call.effects, PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS
    )


def test_noncanonical_import_policy_exposes_possible_identity() -> None:
    index = analyze_python_source_bindings(
        "import importlib\nimportlib.import_module('pkg.leaf')\n",
        policy=PythonBindingPolicy(standard_imports_are_canonical=False),
    )
    call = index.calls[-1]
    assert identity_fact_may_be(
        call.callee_identities, PythonIdentity.IMPORTLIB_IMPORT_MODULE
    )
    assert call.callee_identities & OTHER_IDENTITY


def test_target_python_gates_eager_annotation_effects() -> None:
    source = (
        "from importlib import import_module\n"
        "value: import_module('pkg.annotation') = None\n"
    )
    eager = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=(3, 13))
    )
    deferred = analyze_python_source_bindings(
        source, policy=PythonBindingPolicy(target_python=(3, 14))
    )
    assert len(eager.calls) == 1
    assert deferred.calls == ()


def test_content_cache_is_single_flight_and_filename_independent() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"

    def analyze(index: int):
        return analyze_python_source_bindings(source, filename=f"module_{index}.py")

    with ThreadPoolExecutor(max_workers=8) as executor:
        indexes = list(executor.map(analyze, range(32)))
    assert all(index is indexes[0] for index in indexes)


def test_reparse_query_uses_stable_source_keys_not_ast_identity() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"
    index = analyze_python_source_bindings(source)
    reparsed_call = next(
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.Call)
    )
    fact = index.call_fact(reparsed_call)
    assert fact is not None
    assert fact.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert identity_fact_is_exact(
        index.binding_before(reparsed_call, "importlib") or 0,
        PythonIdentity.IMPORTLIB_MODULE,
    )
