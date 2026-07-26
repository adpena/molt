from __future__ import annotations

import ast
import gc
import weakref
from concurrent.futures import ThreadPoolExecutor
from collections import Counter
from threading import Event

import pytest

from molt.compiler_analysis import python_binding_flow
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


def test_synthetic_node_key_cache_retains_identity_against_id_reuse() -> None:
    analyzer = python_binding_flow._Analyzer(PythonBindingPolicy(), "synthetic")
    first = ast.Name(id="first", lineno=1, col_offset=0)
    first_identity = id(first)
    first_key = analyzer._node_key(first)
    retained = weakref.ref(first)

    del first
    gc.collect()

    retained_node = retained()
    assert retained_node is not None
    assert retained_node in analyzer._node_keys
    second = ast.Name(id="second", lineno=2, col_offset=0)
    assert id(second) != first_identity
    assert analyzer._node_key(second) != first_key


def test_persistent_binding_tree_grows_and_joins_across_radix_boundaries() -> None:
    pool = python_binding_flow._StatePool()
    chunks_per_fixed_depth = 1 << (python_binding_flow._BINDING_TREE_SHIFT * 3)
    chunk_size = python_binding_flow._BINDING_CHUNK_SIZE
    below = (chunks_per_fixed_depth - 1) * chunk_size
    boundary = chunks_per_fixed_depth * chunk_size
    above = (chunks_per_fixed_depth + 1) * chunk_size
    import_module = int(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    dunder_import = int(PythonIdentity.BUILTINS_IMPORT)

    left = pool.set_binding(0, below, import_module)
    left = pool.set_binding(left, boundary, dunder_import)
    right = pool.set_binding(0, above, import_module)
    joined = pool.join(left, right)

    assert pool.binding(left, below) == import_module
    assert pool.binding(left, boundary) == dunder_import
    assert pool.binding(right, above) == import_module
    assert pool.binding(joined, below) & import_module
    assert pool.binding(joined, boundary) & dunder_import
    assert pool.binding(joined, above) & import_module


def test_binding_history_diff_visits_only_changed_trie_branches() -> None:
    pool = python_binding_flow._StatePool()
    chunk_size = python_binding_flow._BINDING_CHUNK_SIZE
    far_chunk = (1 << (python_binding_flow._BINDING_TREE_SHIFT * 4)) - 1
    far_slot = far_chunk * chunk_size
    import_module = int(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    dunder_import = int(PythonIdentity.BUILTINS_IMPORT)
    base = pool.set_binding(0, 0, import_module)
    base = pool.set_binding(base, far_slot, import_module)
    left = pool.set_binding(base, 0, dunder_import)
    right = pool.set_binding(base, far_slot, dunder_import)
    joined = pool.join(left, right)

    assert pool.changed_slots_between(base, joined) == (0, far_slot)
    assert pool.structural_diff_shared_skips > 0
    assert pool.structural_diff_node_visits < far_chunk.bit_length() * 4


def _identity_on_line(source: str, line: int, identity: PythonIdentity) -> bool:
    index = analyze_python_source_bindings(source)
    return any(
        fact.node.lineno == line and identity_fact_is_exact(fact.identities, identity)
        for fact in index.expressions
    )


def test_alias_chain_preserves_exact_import_module_identity() -> None:
    call = _last_call(
        "import importlib as loader\nload = loader.import_module\nload('pkg.leaf')\n"
    )
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert not call.callee_identities & OTHER_IDENTITY


@pytest.mark.parametrize(
    "source",
    (
        "def deferred():\n"
        "    load('pkg.module_late')\n"
        "from importlib import import_module as load\n",
        "def outer():\n"
        "    def deferred():\n"
        "        load('pkg.enclosing_late')\n"
        "    from importlib import import_module as load\n",
        "def outer():\n"
        "    class DeferredOwner:\n"
        "        def load_later(self):\n"
        "            load('pkg.class_enclosing_late')\n"
        "    from importlib import import_module as load\n",
        "from importlib import import_module as load\n"
        "def outer():\n"
        "    load = print\n"
        "    def deferred():\n"
        "        global load\n"
        "        load('pkg.global')\n",
    ),
)
def test_deferred_import_aliases_use_future_canonical_scope_state(
    source: str,
) -> None:
    call = _last_call(source)
    assert call.possible_import_call_kinds() == ("import_module",)


def test_local_parameter_does_not_inherit_outer_import_identity() -> None:
    call = _last_call(
        "from importlib import import_module as load\n"
        "def deferred(load):\n"
        "    load('pkg.not_imported')\n"
    )
    assert call.possible_import_call_kinds() == ()


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
    assert call.definitely_invalidated_members_after & int(PythonMember.UTIL_FIND_SPEC)


def test_intrinsic_require_alias_has_one_exact_binding_identity() -> None:
    index = analyze_python_source_bindings(
        "from _intrinsics import require_intrinsic as require\n"
        "require('molt_demo', globals())\n"
    )
    call = next(fact for fact in index.calls if fact.node.col_offset == 0)
    assert call.callee_is(PythonIdentity.INTRINSICS_REQUIRE)


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
        f"import importlib\n{conditional}importlib.import_module('pkg.leaf')\n"
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


def test_setattr_and_exec_invalidate_import_identity_without_losing_possibility() -> (
    None
):
    setattr_call = _last_call(
        "import importlib\n"
        "setattr(importlib, 'import_module', replacement)\n"
        "importlib.import_module('pkg.leaf')\n"
    )
    assert setattr_call.callee_identities == OTHER_IDENTITY

    exec_call = _last_call(
        "import importlib\nexec(source)\nimportlib.import_module('pkg.leaf')\n"
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
        f"import importlib\n{mutation}importlib.import_module('pkg.leaf')\n"
    )
    assert call.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    assert call.callee_identities & OTHER_IDENTITY


def test_builtins_import_alias_and_member_mutation_share_one_authority() -> None:
    exact = _last_call("from builtins import __import__ as load\nload('pkg.leaf')\n")
    assert exact.callee_is(PythonIdentity.BUILTINS_IMPORT)

    mutated = _last_call(
        "import builtins\nbuiltins.__import__ = replacement\n__import__('pkg.leaf')\n"
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
        "from importlib import import_module\nimport_module('pkg.leaf')\n"
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


def test_binding_cache_evicts_fifo_in_constant_time_authority() -> None:
    cache = python_binding_flow._BindingIndexCache(max_entries=2)
    indexes = {
        name: python_binding_flow.analyze_python_bindings(
            ast.parse(f"value = {ordinal}\n"),
            source_digest=name,
        )
        for ordinal, name in enumerate(("a", "b", "c"))
    }
    calls: Counter[str] = Counter()

    def fetch(name: str):
        def compute():
            calls[name] += 1
            return indexes[name]

        return cache.get_or_compute((name,), compute)

    assert fetch("a") is indexes["a"]
    assert fetch("b") is indexes["b"]
    assert fetch("c") is indexes["c"]
    assert fetch("b") is indexes["b"]
    assert fetch("a") is indexes["a"]
    assert calls == Counter(a=2, b=1, c=1)


def test_loop_fixpoint_does_not_conflate_identical_storage_with_tainted_binding() -> (
    None
):
    states = python_binding_flow._StatePool()
    exact = states.set_binding(0, 0, int(PythonIdentity.IMPORTLIB_MODULE))
    tainted = states.taint_slots(exact, 1)
    loop_header = states.join(exact, tainted)

    assert states.binding(exact, 0) == int(PythonIdentity.IMPORTLIB_MODULE)
    assert states.binding(loop_header, 0) & OTHER_IDENTITY
    assert not states.equivalent(exact, loop_header)


def test_binding_cache_single_flight_exception_wakes_waiters_and_recovers() -> None:
    cache = python_binding_flow._BindingIndexCache(max_entries=2)
    index = python_binding_flow.analyze_python_bindings(
        ast.parse("value = 1\n"),
        source_digest="recovered",
    )
    entered = Event()
    release = Event()
    waiter_started = Event()
    waiter_returned = Event()
    attempts = 0

    def compute():
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            entered.set()
            assert release.wait(5)
            raise ValueError("first analysis failed")
        return index

    def fetch():
        return cache.get_or_compute(("shared",), compute)

    def wait_for_shared_result():
        waiter_started.set()
        try:
            return fetch()
        finally:
            waiter_returned.set()

    with ThreadPoolExecutor(max_workers=2) as executor:
        owner = executor.submit(fetch)
        assert entered.wait(5)
        waiter = executor.submit(wait_for_shared_result)
        assert waiter_started.wait(5)
        assert not waiter_returned.wait(0.05)
        release.set()
        failures: list[ValueError] = []
        for future in (owner, waiter):
            with pytest.raises(ValueError, match="first analysis failed") as caught:
                future.result()
            failures.append(caught.value)

    assert failures[0] is not failures[1]
    assert failures[0].__traceback__ is not failures[1].__traceback__

    assert attempts == 1
    assert fetch() is index
    assert attempts == 2


def test_policy_context_is_part_of_cache_and_index_identity() -> None:
    source = "import importlib\n"
    linux = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(
            target_sys_platform="linux",
            module_name="pkg.mod",
            module_spec_name="pkg.mod",
            module_is_package=False,
            module_execution_kind="imported",
        ),
    )
    windows = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(
            target_sys_platform="win32",
            module_name="pkg.mod",
            module_spec_name="pkg.mod",
            module_is_package=False,
            module_execution_kind="imported",
        ),
    )

    assert linux is not windows
    assert linux.target_sys_platform == "linux"
    assert windows.target_sys_platform == "win32"
    assert linux.module_name == "pkg.mod"
    assert linux.module_execution_kind == "imported"


def test_reparse_query_uses_stable_source_keys_not_ast_identity() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"
    index = analyze_python_source_bindings(source)
    reparsed_call = next(
        node for node in ast.walk(ast.parse(source)) if isinstance(node, ast.Call)
    )
    fact = index.call_fact(reparsed_call)
    assert fact is not None
    assert fact.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
    reparsed_callee = reparsed_call.func
    callee_fact = index.expression_fact(reparsed_callee)
    assert callee_fact is not None
    assert identity_fact_is_exact(
        callee_fact.identities,
        PythonIdentity.IMPORTLIB_IMPORT_MODULE,
    )


def test_ast_cache_identity_includes_spans_used_by_fact_lookup() -> None:
    source = "import importlib\nimportlib.import_module('pkg.leaf')\n"
    shifted_source = "\n    \n" + source
    tree = ast.parse(source)
    shifted_tree = ast.parse(shifted_source)

    assert python_binding_flow.python_ast_digest(
        tree
    ) != python_binding_flow.python_ast_digest(shifted_tree)
    index = python_binding_flow.analyze_python_bindings(
        tree, source_digest=python_binding_flow.python_ast_digest(tree)
    )
    shifted_index = python_binding_flow.analyze_python_bindings(
        shifted_tree,
        source_digest=python_binding_flow.python_ast_digest(shifted_tree),
    )
    call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call))
    shifted_call = next(
        node for node in ast.walk(shifted_tree) if isinstance(node, ast.Call)
    )

    assert index is not shifted_index
    assert index.call_fact(call) is not None
    assert shifted_index.call_fact(shifted_call) is not None
    assert index.call_fact(shifted_call) is None


def test_default_parameter_retains_possible_dunder_import_identity() -> None:
    source = (
        "def load(name, importer=__import__):\n"
        "    return importer(name)\n"
        "load('pkg.leaf')\n"
    )
    index = analyze_python_source_bindings(source)
    importer_call = next(fact for fact in index.calls if fact.node.lineno == 2)

    assert importer_call.callee_may_be(PythonIdentity.BUILTINS_IMPORT)


@pytest.mark.parametrize(
    "source",
    [
        "import importlib\nfrom typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    importlib = replacement\nimportlib.import_module('live')\n",
        "import importlib\nimport typing as t\nif t.TYPE_CHECKING:\n    importlib = replacement\nimportlib.import_module('live')\n",
        "import importlib\nimport typing_extensions as t\nif t.TYPE_CHECKING:\n    importlib = replacement\nimportlib.import_module('live')\n",
    ],
)
def test_type_checking_dead_branches_share_binding_authority(source: str) -> None:
    call = _last_call(source)
    assert call.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE)
