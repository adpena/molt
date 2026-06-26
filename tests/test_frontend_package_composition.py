"""Regression tests for the frontend visitor-mixin package decomposition (F1).

The SimpleTIRGenerator god-class was decomposed (move-only) into a package of
visitor/lowering mixins composed via MRO. These tests pin the invariants that
make that decomposition correct and keep it from silently regressing:

  * the historical public import surface (``from molt.frontend import ...``) is
    preserved exactly;
  * every extracted mixin is present in the SimpleTIRGenerator MRO and its
    methods resolve on the assembled class;
  * the shared data types live in the ``_types`` leaf (no ``__init__`` <-> mixin
    import cycle);
  * compile_to_tir still produces deterministic IR for a representative corpus.

If a future change moves a method into a new mixin, add that mixin to
EXPECTED_MIXINS; if it re-inlines the class, these tests will flag the loss of
the decomposition.
"""

from __future__ import annotations

import ast
import importlib
import inspect
import json
import sys
from pathlib import Path

import molt.frontend as frontend
from molt.frontend import (
    MoltOp,
    MoltValue,
    SimpleTIRGenerator,
    compile_to_tir,
)
from molt.frontend._protocol import _GeneratorProtocol
from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]
FRONTEND_DIR = ROOT / "src" / "molt" / "frontend"

# The public names that external consumers (the CLI package, debug/ir.py, tests, tools)
# import from molt.frontend. This is the backward-compatibility contract.
PUBLIC_SURFACE = [
    "MoltValue",
    "MoltOp",
    "SimpleTIRGenerator",
    "compile_to_tir",
    "SCCPResult",
    "LoopBoundFact",
    "ClassInfo",
    "FuncInfo",
    "MethodInfo",
    "ActiveException",
    "BuiltinFuncSpec",
    "TryScope",
    "CompatibilityError",
    "CompatibilityReporter",
    "FallbackPolicy",
    "CFGGraph",
    "ControlMaps",
    "build_cfg",
    "normalize_type_hint",
]

# Mixins extracted from SimpleTIRGenerator (extend as more families move).
EXPECTED_MIXINS = [
    "LocalBindingMixin",
    "MidendOptimizationMixin",
    "SerializationMixin",
    "OwnershipLoweringMixin",
    "StringFormattingMixin",
    "RuntimeReferenceMixin",
    "CompileWarningMixin",
    "EmissionCoreMixin",
    "ExpressionPrimitivesMixin",
    "ExceptionLoweringMixin",
    "FunctionLifecycleMixin",
    "FunctionMetadataMixin",
    "ModuleGlobalsMixin",
    "ModuleLifecycleMixin",
    "SemaStateMixin",
    "ImportLoweringMixin",
    "SymbolNamingMixin",
    "AttributeAccessMixin",
    "ClassResolutionMixin",
    "TypeAnnotationMixin",
    "LoopLoweringMixin",
    "AnalysisCollectStaticMixin",
    "AnalysisPatternMixin",
    "AsyncGenVisitorMixin",
    "PatternMatchMixin",
    "CallReductionMixin",
    "CallVisitorMixin",
    "ClassDefVisitorMixin",
    "ComprehensionMixin",
    "ExpressionVisitorMixin",
    "FunctionVisitorMixin",
    "AssignmentStatementVisitorMixin",
    "ControlFlowStatementVisitorMixin",
    "StatementScopeVisitorMixin",
]


def test_public_import_surface_preserved() -> None:
    """Every historically-importable name is still importable from the package."""
    for name in PUBLIC_SURFACE:
        assert hasattr(frontend, name), f"molt.frontend lost public name: {name}"


def test_shared_types_live_in_leaf_module() -> None:
    """Shared dataclasses come from the _types leaf, not __init__ (cycle break)."""
    assert MoltValue.__module__ == "molt.frontend._types"
    assert MoltOp.__module__ == "molt.frontend._types"
    # The leaf must never import back into __init__ at runtime.
    types_src = (ROOT / "src" / "molt" / "frontend" / "_types.py").read_text()
    assert "from molt.frontend import" not in types_src.split("if TYPE_CHECKING")[0]


def test_mixins_present_in_mro() -> None:
    """Each extracted mixin must be in the SimpleTIRGenerator MRO."""
    mro_names = {cls.__name__ for cls in SimpleTIRGenerator.__mro__}
    for mixin in EXPECTED_MIXINS:
        assert mixin in mro_names, f"{mixin} missing from SimpleTIRGenerator MRO"
    # ast.NodeVisitor must remain the dispatch base.
    assert "NodeVisitor" in mro_names


def test_moved_methods_resolve_on_class() -> None:
    """Representative methods from each mixin resolve on the assembled class."""
    # local bindings
    assert hasattr(SimpleTIRGenerator, "_load_local_value")
    assert hasattr(SimpleTIRGenerator, "_store_local_value")
    assert hasattr(SimpleTIRGenerator, "_value_reads_plain_local_binding")
    # midend optimization
    assert hasattr(SimpleTIRGenerator, "_run_ir_midend_passes")
    assert hasattr(SimpleTIRGenerator, "_init_midend_state")
    assert hasattr(SimpleTIRGenerator, "_resolve_midend_function_policy")
    assert hasattr(SimpleTIRGenerator, "_canonicalize_control_aware_ops")
    # serialization
    assert hasattr(SimpleTIRGenerator, "map_ops_to_json")
    assert hasattr(SimpleTIRGenerator, "_scalarize_string_split_fields_json")
    # compile warnings
    assert hasattr(SimpleTIRGenerator, "_prescan_compile_warnings")
    assert hasattr(SimpleTIRGenerator, "_emit_deferred_warnings")
    assert hasattr(SimpleTIRGenerator, "_emit_syntax_warning")
    assert hasattr(SimpleTIRGenerator, "_emit_deprecation_warning")
    # emission core
    assert hasattr(SimpleTIRGenerator, "emit")
    assert hasattr(SimpleTIRGenerator, "_suppress_check_exception")
    assert hasattr(SimpleTIRGenerator, "_bridge_fallback")
    # expression primitives
    assert hasattr(SimpleTIRGenerator, "_emit_expr_list")
    assert hasattr(SimpleTIRGenerator, "_emit_compare_op")
    assert hasattr(SimpleTIRGenerator, "_emit_not")
    assert hasattr(SimpleTIRGenerator, "_parse_molt_buffer_call")
    assert hasattr(SimpleTIRGenerator, "_emit_call_bound_or_func")
    # function lifecycle
    assert hasattr(SimpleTIRGenerator, "_function_contains_locals_call")
    assert hasattr(SimpleTIRGenerator, "_task_closure_size")
    assert hasattr(SimpleTIRGenerator, "start_function")
    assert hasattr(SimpleTIRGenerator, "_capture_function_state")
    assert hasattr(SimpleTIRGenerator, "_emit_return_value")
    assert hasattr(SimpleTIRGenerator, "_emit_function_exception_handler")
    assert hasattr(SimpleTIRGenerator, "resume_function")
    # function metadata
    assert hasattr(SimpleTIRGenerator, "_emit_function_metadata")
    assert hasattr(SimpleTIRGenerator, "_emit_function_default_values")
    assert hasattr(SimpleTIRGenerator, "_function_param_names")
    assert hasattr(SimpleTIRGenerator, "_known_module_function_type_hint")
    assert hasattr(SimpleTIRGenerator, "_emit_builtin_function")
    # module globals
    assert hasattr(SimpleTIRGenerator, "_get_or_emit_module_cache")
    assert hasattr(SimpleTIRGenerator, "_emit_global_get")
    assert hasattr(SimpleTIRGenerator, "_emit_globals_dict")
    assert hasattr(SimpleTIRGenerator, "_emit_globals_builtin_ref")
    assert hasattr(SimpleTIRGenerator, "_init_locals_cache_and_pin")
    # module lifecycle
    assert hasattr(SimpleTIRGenerator, "_emit_module_metadata")
    assert hasattr(SimpleTIRGenerator, "_emit_module_frame_enter")
    assert hasattr(SimpleTIRGenerator, "_reset_module_chunk_state")
    # sema-state population
    assert hasattr(SimpleTIRGenerator, "_module_stable_funcs")
    assert hasattr(SimpleTIRGenerator, "_populate_sema_state")
    # symbol naming
    assert hasattr(SimpleTIRGenerator, "_sanitize_module_name")
    assert hasattr(SimpleTIRGenerator, "module_init_symbol")
    assert hasattr(SimpleTIRGenerator, "_function_symbol")
    assert hasattr(SimpleTIRGenerator, "_register_code_symbol")
    assert hasattr(SimpleTIRGenerator, "_qualname_for_def")
    # attribute access
    assert hasattr(SimpleTIRGenerator, "_emit_module_attr_get")
    assert hasattr(SimpleTIRGenerator, "_emit_module_attr_get_on")
    assert hasattr(SimpleTIRGenerator, "_emit_guarded_getattr")
    assert hasattr(SimpleTIRGenerator, "_emit_guarded_setattr")
    assert hasattr(SimpleTIRGenerator, "_emit_attribute_load")
    assert hasattr(SimpleTIRGenerator, "_emit_attribute_store")
    # class resolution
    assert hasattr(SimpleTIRGenerator, "_class_layout_stable")
    assert hasattr(SimpleTIRGenerator, "_emit_class_ref")
    assert hasattr(SimpleTIRGenerator, "_emit_class_method_func")
    assert hasattr(SimpleTIRGenerator, "_c3_merge")
    assert hasattr(SimpleTIRGenerator, "_class_mro_names")
    assert hasattr(SimpleTIRGenerator, "_resolve_method_info")
    # type annotations and hints
    assert hasattr(SimpleTIRGenerator, "_module_has_future_annotations")
    assert hasattr(SimpleTIRGenerator, "_annotation_to_hint")
    assert hasattr(SimpleTIRGenerator, "_propagate_container_hints")
    assert hasattr(SimpleTIRGenerator, "_emit_function_annotate")
    assert hasattr(SimpleTIRGenerator, "_iterable_element_hint")
    assert hasattr(SimpleTIRGenerator, "_emit_guard_type")
    assert hasattr(SimpleTIRGenerator, "_apply_explicit_hint")
    # loop lowering and loop guards
    assert hasattr(SimpleTIRGenerator, "_emit_for_loop")
    assert hasattr(SimpleTIRGenerator, "_emit_range_loop")
    assert hasattr(SimpleTIRGenerator, "_emit_iter_new")
    assert hasattr(SimpleTIRGenerator, "_emit_hoisted_loop_guards")
    assert hasattr(SimpleTIRGenerator, "_emit_loop_unwind")
    assert hasattr(SimpleTIRGenerator, "_visit_loop_body")
    # pattern_match
    assert hasattr(SimpleTIRGenerator, "visit_Match")
    assert hasattr(SimpleTIRGenerator, "_emit_match_class")
    assert hasattr(SimpleTIRGenerator, "_validate_match_pattern")
    # calls (phase 2)
    assert hasattr(SimpleTIRGenerator, "visit_Call")
    assert hasattr(SimpleTIRGenerator, "_emit_call_args_builder")
    assert hasattr(SimpleTIRGenerator, "_fold_bare_super_static")
    # reducer calls (phase 2 semantic subfamily)
    assert hasattr(SimpleTIRGenerator, "_emit_sum_call")
    assert hasattr(SimpleTIRGenerator, "_try_emit_inline_sum_genexpr")
    assert hasattr(SimpleTIRGenerator, "_emit_any_all_call")
    # classes (phase 2)
    assert hasattr(SimpleTIRGenerator, "visit_ClassDef")
    assert hasattr(SimpleTIRGenerator, "_compute_method_closure")
    assert hasattr(SimpleTIRGenerator, "_extract_inline_init_assigns")
    assert hasattr(SimpleTIRGenerator, "_function_needs_classcell")
    # comprehensions
    assert hasattr(SimpleTIRGenerator, "visit_ListComp")
    assert hasattr(SimpleTIRGenerator, "visit_GeneratorExp")
    # functions
    assert hasattr(SimpleTIRGenerator, "visit_FunctionDef")
    assert hasattr(SimpleTIRGenerator, "_has_gpu_kernel_decorator")
    assert hasattr(SimpleTIRGenerator, "visit_Lambda")
    assert hasattr(SimpleTIRGenerator, "visit_Return")
    # async/generator
    assert hasattr(SimpleTIRGenerator, "visit_AsyncFunctionDef")
    assert hasattr(SimpleTIRGenerator, "visit_AsyncFor")
    assert hasattr(SimpleTIRGenerator, "visit_AsyncWith")
    assert hasattr(SimpleTIRGenerator, "visit_Await")
    assert hasattr(SimpleTIRGenerator, "visit_Yield")
    assert hasattr(SimpleTIRGenerator, "visit_YieldFrom")
    # expressions
    assert hasattr(SimpleTIRGenerator, "visit_Name")
    assert hasattr(SimpleTIRGenerator, "visit_BinOp")
    assert hasattr(SimpleTIRGenerator, "visit_TemplateStr")
    assert hasattr(SimpleTIRGenerator, "visit_BoolOp")
    # statement subfamilies
    assert hasattr(SimpleTIRGenerator, "visit_Module")
    assert hasattr(SimpleTIRGenerator, "visit_ImportFrom")
    assert hasattr(SimpleTIRGenerator, "visit_Assign")
    assert hasattr(SimpleTIRGenerator, "visit_AugAssign")
    assert hasattr(SimpleTIRGenerator, "_emit_assign_target")
    assert hasattr(SimpleTIRGenerator, "_emit_unpack_assign")
    assert hasattr(SimpleTIRGenerator, "_emit_delete_name")
    assert hasattr(SimpleTIRGenerator, "_augassign_op_kind")
    assert hasattr(SimpleTIRGenerator, "visit_For")
    assert hasattr(SimpleTIRGenerator, "visit_TryStar")
    # exception lowering and unwind custody
    assert hasattr(SimpleTIRGenerator, "_emit_exception_new")
    assert hasattr(SimpleTIRGenerator, "_emit_exception_match")
    assert hasattr(SimpleTIRGenerator, "_emit_raise_exit")
    assert hasattr(SimpleTIRGenerator, "_emit_raise_if_pending")
    assert hasattr(SimpleTIRGenerator, "_emit_control_flow_scope_unwind")
    assert hasattr(SimpleTIRGenerator, "_emit_sync_try_except_split")
    # ownership and serialization finalization
    assert hasattr(SimpleTIRGenerator, "_emit_inc_ref")
    assert hasattr(SimpleTIRGenerator, "_emit_drop_owned_value")
    assert hasattr(SimpleTIRGenerator, "_analyze_borrowing")
    assert hasattr(SimpleTIRGenerator, "_finalize_code_ids")
    assert hasattr(SimpleTIRGenerator, "_ensure_code_slots_init")
    assert hasattr(SimpleTIRGenerator, "to_json")
    # string formatting and template lowering
    assert hasattr(SimpleTIRGenerator, "_try_extract_const_str")
    assert hasattr(SimpleTIRGenerator, "_parse_format_tokens")
    assert hasattr(SimpleTIRGenerator, "_emit_format_tokens")
    assert hasattr(SimpleTIRGenerator, "_emit_format_spec_value")
    assert hasattr(SimpleTIRGenerator, "_emit_template_interpolation")
    # runtime and intrinsic references
    assert hasattr(SimpleTIRGenerator, "_emit_const_value")
    assert hasattr(SimpleTIRGenerator, "_emit_runtime_function")
    assert hasattr(SimpleTIRGenerator, "_emit_intrinsic_function")
    assert hasattr(SimpleTIRGenerator, "_maybe_record_local_intrinsic_wrapper")
    assert hasattr(SimpleTIRGenerator, "_name_resolves_to_builtin")
    # import lowering
    assert hasattr(SimpleTIRGenerator, "_resolve_relative_import")
    assert hasattr(SimpleTIRGenerator, "_should_attempt_runtime_module_import")
    assert hasattr(SimpleTIRGenerator, "_emit_import_transaction")
    assert hasattr(SimpleTIRGenerator, "_emit_module_load")
    assert hasattr(SimpleTIRGenerator, "_emit_module_import_from_value")
    assert hasattr(SimpleTIRGenerator, "_emit_import_guard")
    assert hasattr(SimpleTIRGenerator, "_source_imports_use_transaction")
    # analysis helpers
    assert hasattr(SimpleTIRGenerator, "_collect_assigned_names")
    assert hasattr(SimpleTIRGenerator, "_collect_free_vars")
    assert hasattr(SimpleTIRGenerator, "_match_vector_reduction_loop")
    assert hasattr(SimpleTIRGenerator, "_match_taq_ingest_loop_body")


def test_mixin_modules_import_standalone() -> None:
    """Mixin modules import without triggering an __init__ <-> mixin cycle."""
    for mod in (
        "molt.frontend._types",
        "molt.frontend.lowering.analysis_collect_static",
        "molt.frontend.lowering.analysis_patterns",
        "molt.frontend.lowering.attribute_access",
        "molt.frontend.lowering.class_resolution",
        "molt.frontend.lowering.compile_warnings",
        "molt.frontend.lowering.emission_core",
        "molt.frontend.lowering.exception_lowering",
        "molt.frontend.lowering.expression_primitives",
        "molt.frontend.lowering.function_lifecycle",
        "molt.frontend.lowering.function_metadata",
        "molt.frontend.lowering.import_lowering",
        "molt.frontend.lowering.local_bindings",
        "molt.frontend.lowering.loop_lowering",
        "molt.frontend.lowering.midend_optimization",
        "molt.frontend.lowering.module_lifecycle",
        "molt.frontend.lowering.ownership_lowering",
        "molt.frontend.lowering.serialization",
        "molt.frontend.lowering.sema_state",
        "molt.frontend.lowering.string_formatting",
        "molt.frontend.lowering.symbol_naming",
        "molt.frontend.lowering.type_annotations",
        "molt.frontend.visitors.async_gen",
        "molt.frontend.visitors.pattern_match",
        "molt.frontend.visitors.call_reductions",
        "molt.frontend.visitors.calls",
        "molt.frontend.visitors.classes",
        "molt.frontend.visitors.comprehensions",
        "molt.frontend.visitors.expressions",
        "molt.frontend.visitors.functions",
        "molt.frontend.visitors.statement_assignments",
        "molt.frontend.visitors.statement_control_flow",
        "molt.frontend.visitors.statement_scope",
    ):
        assert importlib.import_module(mod) is not None


def test_reducer_call_lowering_stays_out_of_call_dispatcher() -> None:
    """Reducer-specific fusion belongs to call_reductions, not calls.py."""
    calls_src = (
        ROOT / "src" / "molt" / "frontend" / "visitors" / "calls.py"
    ).read_text(encoding="utf-8")
    reductions_src = (
        ROOT / "src" / "molt" / "frontend" / "visitors" / "call_reductions.py"
    ).read_text(encoding="utf-8")

    assert "class CallReductionMixin" in reductions_src
    assert "def _emit_sum_call" in reductions_src
    assert "def _emit_any_all_call" in reductions_src
    assert "def _try_emit_inline_sum_genexpr" in reductions_src
    assert "def _emit_sum_call" not in calls_src
    assert "def _emit_any_all_call" not in calls_src
    assert "def _try_emit_inline_sum_genexpr" not in calls_src
    assert "return self._emit_sum_call(func_id, node, needs_bind)" in calls_src
    assert "return self._emit_any_all_call(func_id, node, needs_bind)" in calls_src


# ---------------------------------------------------------------------------
# Protocol-drift guard (phase-1 reviewer finding)
#
# Each mixin annotates ``self`` as ``_GeneratorProtocol`` under TYPE_CHECKING so
# cross-mixin ``self.<method>`` / ``self.<attr>`` references type-check across
# files.  That guarantee only holds while the Protocol is a SUPERSET of the
# assembled generator's real surface.  If a method moves into a mixin but the
# Protocol is not regenerated (tools/gen_protocol.py), the moved method - and
# every sibling-mixin call to it - silently loses static checking.  These tests
# fail the moment the Protocol and the assembled class diverge.
# ---------------------------------------------------------------------------

# Names provided by object that are NOT part of the generator's own protocol
# surface. Project-owned methods that shadow ast.NodeVisitor helpers (for
# example visit_Constant) must remain on the protocol; only untouched
# NodeVisitor helpers are filtered at collection time.
_NODE_VISITOR_DISPATCH_METHODS = {"generic_visit", "visit"}
_BUILTIN_NAMES = set(dir(object))


def _protocol_methods() -> set[str]:
    return {
        name
        for name, val in vars(_GeneratorProtocol).items()
        if callable(val) and not name.startswith("__")
    }


def _protocol_attrs() -> set[str]:
    attrs: set[str] = set()
    for klass in _GeneratorProtocol.__mro__:
        attrs.update(getattr(klass, "__annotations__", {}))
    return attrs


def _assembled_class_methods() -> set[str]:
    """Public methods contributed by SimpleTIRGenerator, its mixins, and the
    NodeVisitor dispatcher methods used by those mixins."""
    names: set[str] = set()
    for klass in SimpleTIRGenerator.__mro__:
        if klass is object:
            continue
        for name, val in vars(klass).items():
            if name.startswith("__"):
                continue
            if klass is ast.NodeVisitor and name not in _NODE_VISITOR_DISPATCH_METHODS:
                continue
            if callable(val):
                names.add(name)
    return names - _BUILTIN_NAMES


def _direct_self_store_attrs(func: object) -> set[str]:
    """Direct ``self.x = ...`` stores in one generator/mixin method.

    Nested helper classes/functions define their own ``self`` and are not part
    of the assembled generator state surface.
    """
    import textwrap

    attrs: set[str] = set()
    try:
        src = textwrap.dedent(inspect.getsource(func))
    except (OSError, TypeError):
        return attrs
    module = ast.parse(src)
    method = next(
        (
            n
            for n in module.body
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
        ),
        None,
    )
    if method is None or not method.args.args or method.args.args[0].arg != "self":
        return attrs

    class Visitor(ast.NodeVisitor):
        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            if node is method:
                self.generic_visit(node)

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            if node is method:
                self.generic_visit(node)

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            return

        def visit_Lambda(self, node: ast.Lambda) -> None:
            return

        def visit_Attribute(self, node: ast.Attribute) -> None:
            if (
                isinstance(node.value, ast.Name)
                and node.value.id == "self"
                and isinstance(node.ctx, ast.Store)
            ):
                attrs.add(node.attr)
            self.generic_visit(node)

    Visitor().visit(method)
    return attrs


def _assembled_class_attrs() -> set[str]:
    """Instance attributes assigned by direct ``self.x = ...`` stores plus
    class-level annotated vars across the assembled class and its mixins."""
    attrs: set[str] = set()
    for klass in SimpleTIRGenerator.__mro__:
        if klass is object:
            continue
        for value in vars(klass).values():
            if isinstance(value, (staticmethod, classmethod)):
                value = value.__func__
            attrs.update(_direct_self_store_attrs(value))
    for klass in SimpleTIRGenerator.__mro__:
        if klass is object:
            continue
        attrs.update(getattr(klass, "__annotations__", {}))
    return attrs - _BUILTIN_NAMES


def _discover_mixin_classes() -> dict[str, type]:
    """Auto-discover every *Mixin class under visitors/ and lowering/ so this
    guard automatically covers new mixins added in later extraction phases."""
    found: dict[str, type] = {}
    for sub in ("visitors", "lowering"):
        pkg_dir = FRONTEND_DIR / sub
        for path in sorted(pkg_dir.glob("*.py")):
            if path.name == "__init__.py":
                continue
            mod = importlib.import_module(f"molt.frontend.{sub}.{path.stem}")
            for name, obj in vars(mod).items():
                if (
                    isinstance(obj, type)
                    and name.endswith("Mixin")
                    and obj.__module__ == mod.__name__
                ):
                    found[name] = obj
    return found


def test_protocol_covers_full_class_method_surface() -> None:
    """_GeneratorProtocol must declare every method the assembled class exposes.

    A missing entry means the Protocol drifted from the class
    (tools/gen_protocol.py was not re-run after a move), so sibling-mixin
    ``self.<method>`` calls no longer type-check.
    """
    missing = _assembled_class_methods() - _protocol_methods()
    assert not missing, (
        "Protocol drift: methods on SimpleTIRGenerator missing from "
        f"_GeneratorProtocol (re-run tools/gen_protocol.py): {sorted(missing)}"
    )


def test_protocol_covers_full_class_attr_surface() -> None:
    """_GeneratorProtocol must declare every instance/class attribute the
    assembled generator sets, so cross-mixin ``self.<attr>`` reads type-check."""
    missing = _assembled_class_attrs() - _protocol_attrs()
    assert not missing, (
        "Protocol drift: attributes on SimpleTIRGenerator missing from "
        f"_GeneratorProtocol (re-run tools/gen_protocol.py): {sorted(missing)}"
    )


def test_protocol_generator_is_idempotent() -> None:
    """The tracked protocol generator must reproduce the checked-in files."""
    proc = run_guarded_test_process(
        [sys.executable, str(ROOT / "tools" / "gen_protocol.py"), "--check"],
        prefix="MOLT_TEST_SUITE",
        cwd=ROOT,
        check=False,
    )
    assert proc.returncode == 0, proc.stdout + proc.stderr


def test_every_mixin_method_is_on_protocol() -> None:
    """Per-mixin view of the same contract, for precise failure attribution.

    Each extracted mixin's own (non-dunder) methods must all appear on the
    Protocol — every mixin annotates ``self`` as the Protocol, so any of its
    methods that is absent is invisible to every other mixin's static checks.
    """
    proto = _protocol_methods()
    mixins = _discover_mixin_classes()
    assert mixins, "no *Mixin classes discovered under visitors/ or lowering/"
    drift: dict[str, list[str]] = {}
    for mixin_name, mixin_cls in sorted(mixins.items()):
        own = {
            name
            for name, val in vars(mixin_cls).items()
            if callable(val) and not name.startswith("__")
        }
        missing = sorted(own - proto)
        if missing:
            drift[mixin_name] = missing
    assert not drift, (
        "Protocol drift: mixin methods missing from _GeneratorProtocol "
        f"(re-run tools/gen_protocol.py): {drift}"
    )


def test_discovered_mixins_match_expected() -> None:
    """The auto-discovered mixin set must equal EXPECTED_MIXINS, so adding a
    mixin without registering it (or vice versa) is caught."""
    discovered = set(_discover_mixin_classes())
    assert discovered == set(EXPECTED_MIXINS), (
        f"mixin registry drift: discovered={sorted(discovered)} "
        f"expected={sorted(EXPECTED_MIXINS)}"
    )


def test_compile_to_tir_deterministic_with_match() -> None:
    """A match-statement program compiles deterministically through the mixins."""
    source = (
        "def classify(x):\n"
        "    match x:\n"
        "        case [a, b]:\n"
        "            return a + b\n"
        "        case {'k': v}:\n"
        "            return v\n"
        "        case int():\n"
        "            return x\n"
        "        case _:\n"
        "            return None\n"
    )
    ir_a = json.dumps(compile_to_tir(source), sort_keys=True)
    ir_b = json.dumps(compile_to_tir(source), sort_keys=True)
    assert ir_a == ir_b
    # The match lowering must have produced ops (sanity: the mixin ran).
    assert "classify" in ir_a
