"""Source-pure import requests and execution demands; topology is completed separately."""

from __future__ import annotations

import ast
from molt.python_private_names import (
    python_import_binding,
    python_source_field,
)
from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Literal, NamedTuple, cast

from molt.compiler_analysis.python_lexical_scope import (
    python_execution_regions,
    definition_lexical_regions,
    python_scope_declarations,
    module_annotation_execution,
)
from molt.compiler_analysis.python_imports import (
    SourceImportSite,
    SourceAllExports,
)
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from molt.cli.models import ImportScanMode
from molt.compiler_analysis.python_imports import (
    RuntimeCatalogDeferral,
    _ImportDiscoveryProjection,
    _ImportScanRequests,
    _SourceValueDemand,
    _SourceImportBinding,
    _SourceValueSummary,
    _StaticSourceExecutionRequest,
    _StaticSourcePath,
)
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)
from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    SourceDemandPhase,
    static_expression_result,
)
from molt.compiler_analysis.python_binding_facts import (
    PythonParameterRef,
)
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_namespace_writes import (
    python_namespace_summary,
    python_package_name,
)
from molt.compiler_analysis.python_identity_catalog_generated import (
    ALL_INVALID_MEMBERS,
    PythonIdentity,
)
from molt.compiler_analysis.python_binding_facts import PythonBindingIndex
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_binding_facts,
    analyze_python_bindings,
)
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    StaticImportPlan,
    bind_static_import_call_arguments,
    dunder_globals_state_from_expression,
    metadata_value_from_expression,
    module_import_context_with_metadata_proof,
    plan_static_import_request,
    require_static_import_modules,
    static_import_candidates,
    static_import_discovery,
    source_import_requests_from_expressions,
    static_import_level_from_result,
    static_import_fromlist_is_empty,
    UnresolvedStaticImportError,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    python_node_source_key,
    python_pattern_capture_names,
    _PythonAstDigestAdmission,
)


_IMPORT_SCAN_MODES = frozenset({"full", "module_init"})


def _source_binding_policy(
    *,
    target_python: TargetPythonVersion,
    module_name: str | None = None,
    is_package: bool = False,
    namespace_premise: PythonNamespacePremise | None = None,
    include_import_discovery: bool = False,
) -> PythonBindingPolicy:
    return PythonBindingPolicy(
        target_python=target_python.feature_version,
        module_name=module_name,
        module_spec_name=module_name,
        module_is_package=is_package,
        module_execution_kind="script" if module_name is None else "imported",
        include_import_discovery=include_import_discovery,
        foreign_namespace_writes=(
            namespace_premise.foreign_writes if namespace_premise is not None else None
        ),
        callback_member_writes=(
            namespace_premise.member_writes
            if namespace_premise is not None
            else ALL_INVALID_MEMBERS
        ),
        entry_invalid_members=(
            namespace_premise.member_writes if namespace_premise is not None else 0
        ),
    )


def _module_import_scan_mode(module_name: str, *, full_scan: bool) -> ImportScanMode:
    """Explicit roots own full scans; imported sources start at initialization."""
    return "full" if full_scan else "module_init"


_DYNAMIC_RELATIVE_ANCHOR_ERRORS = frozenset(
    {"unknown_package", "unknown_spec", "unknown_name"}
)


@dataclass(slots=True)
class _DynamicRelativeImportDiscovery:
    """Graph candidates for imports whose runtime argument/anchor is dynamic."""

    candidates: list[str] = field(default_factory=list)
    seen: set[str] = field(default_factory=set)
    required: bool = False

    def record(
        self,
        request: StaticImportRequest,
        contexts: Sequence[ModuleImportContext],
        *,
        lexical_request: StaticImportRequest | None = None,
    ) -> tuple[str, ...]:
        self.required = True
        candidates = static_import_discovery(
            request, contexts, lexical_request=lexical_request
        ).modules
        for candidate in candidates:
            if candidate not in self.seen:
                self.seen.add(candidate)
                self.candidates.append(candidate)
        return candidates

    def projection(
        self,
        imports: Collection[str],
        star_modules: Collection[str],
        dynamic_star_modules: Collection[str],
    ) -> _ImportDiscoveryProjection:
        return _ImportDiscoveryProjection(
            tuple(imports),
            tuple(self.candidates),
            self.required,
            tuple(dict.fromkeys(star_modules)),
            tuple(dict.fromkeys(dynamic_star_modules)),
        )

    def record_call_binding(self, name: str | None) -> None:
        self.required = True
        # This is only a discovery candidate: expanded level/package/fromlist
        # operands can select another catalog row or fail before import. Never
        # substitute a lexical package for an unresolved relative call.
        if name and not name.startswith(".") and name not in self.seen:
            self.seen.add(name)
            self.candidates.append(name)


class _SealedImports(NamedTuple):
    modules: tuple[str, ...]
    # The runtime import catalog completes these modules: owner scans only.
    runtime_catalog: bool = False


@dataclass(slots=True)
class _RuntimeCatalogUse:
    """Which collections of a runtime-import owner's scan the catalog completes."""

    imports: bool = False
    star_modules: bool = False

    def deferral(self, source_ast_digest: str) -> RuntimeCatalogDeferral:
        return RuntimeCatalogDeferral(
            source_ast_digest, self.imports, self.star_modules
        )


def _sealed_import_modules(
    request: StaticImportRequest,
    contexts: Sequence[ModuleImportContext],
    *,
    runtime_catalog_owner: bool = False,
    dynamic_relative_import_discovery: _DynamicRelativeImportDiscovery | None = None,
    lexical_discovery_request: StaticImportRequest | None = None,
    lexical_discovery_contexts: Sequence[ModuleImportContext] = (),
    source_discovery_requests: Sequence[
        tuple[StaticImportRequest, ModuleImportContext]
    ] = (),
) -> _SealedImports:
    module_name = contexts[0].module_name if contexts else None
    plan = plan_static_import_request(request, contexts)
    unclassified_errors = tuple(
        error for error in plan.errors if error not in _DYNAMIC_RELATIVE_ANCHOR_ERRORS
    )
    # Calls and the runtime's two relative-statement ImportErrors remain
    # catchable under exact custody. Other statement errors stay build-fatal.
    runtime_resolution_errors = bool(plan.errors) and (
        request.kind != "statement"
        or request.level > 0
        and all(
            error in _DYNAMIC_RELATIVE_ANCHOR_ERRORS
            or error in {"no_parent", "beyond_top"}
            for error in plan.errors
        )
    )
    needs_runtime_custody = plan.requires_runtime or runtime_resolution_errors
    if plan.requires_runtime and unclassified_errors and not runtime_resolution_errors:
        raise UnresolvedStaticImportError(
            "module import scanner "
            f"({module_name or '<script>'}: {request.name!r}) cannot resolve import: "
            + ", ".join(unclassified_errors)
        )
    if plan.requires_runtime and dynamic_relative_import_discovery is not None:
        # A rejected storage proof does not erase source-state possibilities.
        # Keep those roots in the same discovery-only projection as the fresh
        # source-owner twin; neither grants execution metadata authority.
        if lexical_discovery_contexts:
            dynamic_relative_import_discovery.record(
                request, lexical_discovery_contexts
            )
        for source_request, source_context in source_discovery_requests:
            dynamic_relative_import_discovery.record(source_request, (source_context,))
        dynamic_relative_import_discovery.record(
            request,
            contexts,
            lexical_request=lexical_discovery_request,
        )
    if runtime_resolution_errors and dynamic_relative_import_discovery is not None:
        dynamic_relative_import_discovery.required = True
    if needs_runtime_custody and runtime_catalog_owner:
        # Do not replace poisoned metadata with the lexical package. The runtime
        # keeps Python's relative-import semantics and may select any retained
        # catalog row (or fail closed if the requested module is not admitted);
        # completion appends that live catalog.
        return _SealedImports(plan.modules, True)
    if needs_runtime_custody and dynamic_relative_import_discovery is not None:
        # Graph discovery retains only statically proven semantic alternatives
        # here. Lexical owner candidates travel in a distinct projection and
        # cannot become a runtime relative-import fallback.
        return _SealedImports(plan.modules)
    return _SealedImports(
        require_static_import_modules(
            plan,
            consumer=(
                f"module import scanner ({module_name or '<script>'}: {request.name!r})"
            ),
        )
    )


@dataclass(frozen=True, slots=True)
class _StaticImportCallPayload:
    call: ast.Call
    target: str
    name: ast.expr | None
    package: ast.expr | None = None
    globals: ast.expr | None = None
    fromlist: ast.expr | None = None
    level: ast.expr | None = None
    requires_runtime_binding: bool = False


_STATIC_SOURCE_LOADER_TARGETS = frozenset(
    {
        "importlib.util.spec_from_file_location",
        "importlib.machinery.SourceFileLoader",
        "importlib.machinery.SourcelessFileLoader",
    }
)


_STATIC_SOURCE_EXECUTION_MARKERS = (
    "spec_from_file_location",
    "SourceFileLoader",
    "SourcelessFileLoader",
    "run_path",
)


_STATIC_SOURCE_PATH_JOIN_OPERATIONS: Mapping[
    str, Literal["os_join", "posix_join", "nt_join"]
] = {
    "os.path.join": "os_join",
    "posixpath.join": "posix_join",
    "ntpath.join": "nt_join",
}


def _source_may_use_static_source_execution(source: str) -> bool:
    return any(marker in source for marker in _STATIC_SOURCE_EXECUTION_MARKERS)


def _collect_static_source_execution_requests(
    tree: ast.AST,
    *,
    source_path: Path,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    import_scan_mode: ImportScanMode = "full",
    module_name: str | None = None,
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[_StaticSourceExecutionRequest, ...]:
    """Collect statically addressable loader/runpy source execution roots.

    This is the source-path projection of the import scanner.  It deliberately
    accepts only expressions that can be evaluated without executing user code;
    dynamic paths remain runtime capability work and never become build inputs
    by accident.
    """

    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    import_flow = analyze_python_bindings(
        cast(ast.Module, tree),
        source_digest=ast_digest_admission.digest,
        ast_digest_admission=ast_digest_admission,
        policy=_source_binding_policy(
            target_python=target_python,
            module_name=module_name,
            is_package=source_path.name == "__init__.py",
            namespace_premise=namespace_premise,
        ),
    ).module_import_flow
    aliases: dict[str, str] = {
        "importlib": "importlib",
        "runpy": "runpy",
        "Path": "pathlib.Path",
    }
    constants: dict[str, str | _StaticSourcePath] = {}

    if isinstance(tree, ast.Module):
        for stmt in ast_digest_admission.module_setup_nodes:
            if not import_flow.states_for(stmt):
                continue
            if isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    bound = python_import_binding(alias)
                    aliases[bound] = (
                        alias.name if alias.asname else alias.name.partition(".")[0]
                    )
            elif isinstance(stmt, ast.ImportFrom) and stmt.level == 0 and stmt.module:
                for alias in stmt.names:
                    aliases[alias.asname or alias.name] = f"{stmt.module}.{alias.name}"

    def qualified_name(expr: ast.expr) -> str | None:
        if isinstance(expr, ast.Name):
            return aliases.get(expr.id, expr.id)
        if isinstance(expr, ast.Attribute):
            base = qualified_name(expr.value)
            return None if base is None else f"{base}.{expr.attr}"
        return None

    def static_value(expr: ast.expr) -> str | _StaticSourcePath | None:
        if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
            return expr.value
        if isinstance(expr, ast.Name):
            return constants.get(expr.id)
        if isinstance(expr, ast.BinOp):
            left = static_value(expr.left)
            right = static_value(expr.right)
            if (
                isinstance(expr.op, ast.Add)
                and isinstance(left, str)
                and isinstance(right, str)
            ):
                return left + right
            if (
                isinstance(expr.op, ast.Div)
                and isinstance(left, (str, _StaticSourcePath))
                and isinstance(right, (str, _StaticSourcePath))
            ):
                return _StaticSourcePath("join", (left, right))
            return None
        if isinstance(expr, ast.Call):
            target = qualified_name(expr.func)
            if target == "pathlib.Path" and len(expr.args) == 1 and not expr.keywords:
                value = static_value(expr.args[0])
                return (
                    _StaticSourcePath("path", (value,)) if value is not None else None
                )
            if (
                target is not None
                and target in _STATIC_SOURCE_PATH_JOIN_OPERATIONS
                and expr.args
            ):
                parts = [static_value(arg) for arg in expr.args]
                if all(part is not None for part in parts):
                    return _StaticSourcePath(
                        _STATIC_SOURCE_PATH_JOIN_OPERATIONS[target],
                        tuple(part for part in parts if part is not None),
                    )
            if (
                isinstance(expr.func, ast.Attribute)
                and expr.func.attr in {"resolve", "absolute"}
                and not expr.args
                and not expr.keywords
            ):
                value = static_value(expr.func.value)
                if value is not None:
                    return _StaticSourcePath(
                        "resolve" if expr.func.attr == "resolve" else "absolute",
                        (value,),
                    )
        return None

    # Module constants are the common authority for loader paths and remain
    # visible to calls nested in entry-module functions.
    if isinstance(tree, ast.Module):
        for stmt in ast_digest_admission.module_setup_nodes:
            if not import_flow.states_for(stmt):
                continue
            assignment: tuple[ast.expr, ast.expr] | None = None
            if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
                assignment = stmt.targets[0], stmt.value
            elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
                assignment = stmt.target, stmt.value
            if assignment is None or not isinstance(assignment[0], ast.Name):
                continue
            value = static_value(assignment[1])
            if value is not None:
                constants[assignment[0].id] = value

    def call_argument(call: ast.Call, position: int, keyword: str) -> ast.expr | None:
        if position < len(call.args):
            return call.args[position]
        return next((item.value for item in call.keywords if item.arg == keyword), None)

    requests: list[_StaticSourceExecutionRequest] = []
    seen: set[_StaticSourceExecutionRequest] = set()
    for node in _scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        module_name=module_name,
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=ast_digest_admission,
    ):
        if not isinstance(node, ast.Call):
            continue
        if not import_flow.states_for(node):
            continue
        target = qualified_name(node.func)
        request_name: str | None
        path_expr: ast.expr | None
        if target in _STATIC_SOURCE_LOADER_TARGETS:
            name_expr = call_argument(node, 0, "name")
            path_expr = call_argument(node, 1, "location")
            if target != "importlib.util.spec_from_file_location":
                path_expr = call_argument(node, 1, "path")
            name_value = static_value(name_expr) if name_expr is not None else None
            if not isinstance(name_value, str):
                continue
            request_name = name_value
        elif target == "runpy.run_path":
            request_name = None
            path_expr = call_argument(node, 0, "path_name")
        else:
            continue
        path_value = static_value(path_expr) if path_expr is not None else None
        if path_value is None:
            continue
        request = _StaticSourceExecutionRequest(request_name, path_value)
        if request not in seen:
            seen.add(request)
            requests.append(request)
    return tuple(requests)


def _validate_import_scan_mode(import_scan_mode: ImportScanMode) -> None:
    if import_scan_mode not in _IMPORT_SCAN_MODES:
        raise ValueError(f"unknown import scan mode: {import_scan_mode}")


def _source_body(
    tree: ast.AST, owner: PythonSourceKey | None, admission: _PythonAstDigestAdmission
) -> tuple[ast.AST, ...]:
    if owner is None:
        return tuple(getattr(tree, "body", ()))
    node = admission.deferred_nodes.get(owner)
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        return tuple(node.body)
    if isinstance(node, ast.Lambda):
        return (node.body,)
    if isinstance(node, ast.GeneratorExp):
        return definition_lexical_regions(node, eager_annotations=False).body
    if isinstance(node, getattr(ast, "TypeAlias", ())):
        return tuple(
            value
            for parameter in node.type_params
            for field in ("bound", "default_value")
            if isinstance(value := getattr(parameter, field, None), ast.expr)
        ) + (node.value,)
    raise ValueError(f"deferred demand lost source owner: {owner!r}")


def _source_parameter_bindings(admission, source_owner, source_arguments):
    if not source_arguments:
        return {}
    node = admission.deferred_nodes.get(source_owner)
    if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
        raise ValueError("argument facts require a function source owner")
    # Bind proven positional member-selection arguments. Expired or rebound
    # parameter facts remain unknown in _source_expression_result.
    parameters = (*node.args.posonlyargs, *node.args.args)
    if len(source_arguments) > len(parameters):
        return {}
    return dict(zip((parameter.arg for parameter in parameters), source_arguments))


def _source_expression_result(binding_index, node, bindings):
    value = binding_index.static_value(node)
    if isinstance(value, PythonParameterRef) and value.name in bindings:
        return StaticExpressionResult.scalar(
            bindings[value.name], evaluation_required=True
        )
    result = binding_index.expression_result(node)
    if (
        bindings
        and isinstance(
            node, (ast.UnaryOp, ast.BinOp, ast.BoolOp, ast.Compare, ast.IfExp)
        )
        and not result.value_known
    ):
        return static_expression_result(
            node,
            fact_result=lambda child: _source_expression_result(
                binding_index, child, bindings
            ),
        )
    return result


def _static_scan_nodes(
    tree: ast.AST,
    *,
    include_function_bodies: bool,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    if not isinstance(tree, ast.Module):
        return tuple(ast.walk(tree))
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    binding_index = analyze_python_binding_facts(
        tree,
        source_digest=ast_digest_admission.digest,
        ast_digest_admission=ast_digest_admission,
        policy=_source_binding_policy(
            target_python=target_python, namespace_premise=namespace_premise
        ).flow_policy(),
    )
    source_bindings = _source_parameter_bindings(
        ast_digest_admission, source_owner, source_arguments
    )
    return tuple(
        region.node
        for region in python_execution_regions(
            tree,
            target_python=target_python.feature_version,
            include_deferred=include_function_bodies,
            source_owner=source_owner,
            body=_source_body(tree, source_owner, ast_digest_admission),
            fact_result=lambda expression: _source_expression_result(
                binding_index, expression, source_bindings
            ),
        )
        if region.phase == "evaluate"
    )


def _module_init_scan_nodes(
    tree: ast.AST,
    *,
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(
        tree,
        include_function_bodies=False,
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=ast_digest_admission,
    )


def _full_static_scan_nodes(
    tree: ast.AST,
    *,
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    return _static_scan_nodes(
        tree,
        include_function_bodies=True,
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=ast_digest_admission,
    )


def _scan_nodes_for_import_mode(
    tree: ast.AST,
    import_scan_mode: ImportScanMode,
    *,
    module_name: str | None = None,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> tuple[ast.AST, ...]:
    _validate_import_scan_mode(import_scan_mode)
    return _static_scan_nodes(
        tree,
        include_function_bodies=import_scan_mode == "full" and source_owner is None,
        source_owner=source_owner,
        source_arguments=source_arguments,
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=ast_digest_admission,
    )


def _uses_runtime_import_protocol(
    binding_index: PythonBindingIndex,
    tree: ast.AST,
    *,
    import_scan_mode: ImportScanMode,
    module_name: str | None,
    source_owner: PythonSourceKey | None,
    source_arguments: tuple[str, ...],
    namespace_premise: PythonNamespacePremise | None,
    target_python: TargetPythonVersion,
    ast_digest_admission: _PythonAstDigestAdmission,
) -> bool:
    """Whether reached code needs the runtime import protocol.

    It does when it reaches an import statement (other than ``__future__`` and
    the intrinsics module) or a call that may import or find a module spec.
    The scan answers from its own binding index, so the answer shares the
    scan's policy and costs no second analysis.
    """
    import_flow = binding_index.module_import_flow
    for node in _scan_nodes_for_import_mode(
        tree,
        import_scan_mode,
        module_name=module_name,
        source_owner=source_owner,
        source_arguments=source_arguments,
        namespace_premise=namespace_premise,
        target_python=target_python,
        ast_digest_admission=ast_digest_admission,
    ):
        if not import_flow.states_for(node):
            continue
        if isinstance(node, ast.Import):
            if any(alias.name != "_intrinsics" for alias in node.names):
                return True
        elif isinstance(node, ast.ImportFrom):
            if node.module == "__future__":
                continue
            if node.level == 0 and (
                node.module == "_intrinsics"
                or node.module is not None
                and node.module.endswith("._intrinsics")
            ):
                continue
            return True
        if isinstance(node, ast.Call):
            fact = binding_index.call_fact(node)
            if fact is not None and (
                fact.possible_import_call_kinds()
                or fact.callee_may_be(PythonIdentity.IMPORTLIB_FIND_SPEC)
            ):
                return True
    return False


def _collect_imports(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    import_scan_mode: ImportScanMode = "full",
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    _runtime_catalog: _RuntimeCatalogUse | None = None,
    _source_demands: list[_SourceValueDemand] | None = None,
    _source_bindings: list[_SourceImportBinding] | None = None,
    _source_values: list[_SourceValueSummary] | None = None,
    _source_sites: list[SourceImportSite] | None = None,
    _dynamic_relative_import_discovery: _DynamicRelativeImportDiscovery | None = None,
    _star_modules: list[str] | None = None,
    _dynamic_star_modules: list[str] | None = None,
    _runtime_import_protocol: list[bool] | None = None,
) -> list[str]:
    if _runtime_catalog is not None and (
        source_owner is not None or import_scan_mode != "full"
    ):
        raise ValueError("runtime import custody requires full-depth owner scans")
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    _validate_import_scan_mode(import_scan_mode)
    imports: list[str] = []
    needs_typing = False
    needs_string_templatelib = False
    template_str_cls = getattr(ast, "TemplateStr", None)
    helper_import_calls: dict[
        PythonSourceKey,
        tuple[list[str], set[str], list[_StaticImportCallPayload]],
    ] = {}
    base_import_context = ModuleImportContext(
        module_name,
        is_package,
        spec_name=module_name,
        target_python=target_python.feature_version,
        execution_kind="script" if module_name is None else "imported",
    )
    binding_index = analyze_python_bindings(
        cast(ast.Module, tree),
        source_digest=ast_digest_admission.digest,
        ast_digest_admission=ast_digest_admission,
        policy=_source_binding_policy(
            target_python=target_python,
            module_name=module_name,
            is_package=is_package,
            namespace_premise=namespace_premise,
            include_import_discovery=_dynamic_relative_import_discovery is not None,
        ),
    )
    import_flow = binding_index.module_import_flow
    if _runtime_import_protocol is not None:
        _runtime_import_protocol.append(
            _uses_runtime_import_protocol(
                binding_index,
                tree,
                import_scan_mode=import_scan_mode,
                module_name=module_name,
                source_owner=source_owner,
                source_arguments=source_arguments,
                namespace_premise=namespace_premise,
                target_python=target_python,
                ast_digest_admission=ast_digest_admission,
            )
        )
    if _source_values is not None and source_owner is None:
        _source_values.append(
            _SourceValueSummary(
                binding_index.source_namespaces,
                binding_index.source_attributes,
                binding_index.source_elements,
                binding_index.source_calls,
                binding_index.source_parameters,
                binding_index.source_function_results,
                binding_index.source_bases,
                binding_index.source_completions,
            )
        )

    projecting_helper = False

    def record_site(node, request, contexts=()):
        if _source_sites is None or projecting_helper:
            return
        plan = (
            plan_static_import_request(request, contexts)
            if request is not None
            else StaticImportPlan((), (), True, True)
        )
        owner_plan = (
            plan_static_import_request(replace(request, fromlist=()), contexts)
            if request is not None and request.fromlist
            else plan
        )
        _source_sites.append(
            SourceImportSite(
                python_node_source_key(node),
                request,
                plan,
                owner_plan,
                tuple(
                    (alias.asname or alias.name, python_source_field(alias, "name"))
                    for alias in node.names
                )
                if isinstance(node, ast.ImportFrom)
                else (),
            )
        )

    def demand(
        result: StaticExpressionResult,
        phase: SourceDemandPhase,
        *,
        contents: bool = False,
    ) -> None:
        if _source_demands is not None:
            origins = (
                result.exposed_source_origins if contents else result.source_origins
            )
            if phase == "call":
                factories = {
                    ref.source for ref in result.deferred if ref.phase == "factory"
                }
                origins = origins - factories
            _source_demands.extend(
                _SourceValueDemand(
                    origin,
                    "expose" if phase == "escape" else phase,
                )
                for origin in sorted(origins)
            )
            if phase not in {"read", "factory", "expose"}:
                # Namespace alternatives are value provenance, not an eager
                # read effect. Dispatch consumes them at the selected phase.
                refs = result.exposed_deferred if contents else result.deferred
                _source_demands.extend(
                    _SourceValueDemand(
                        ref.source,
                        "expose" if phase == "escape" else "escape",
                    )
                    for ref in refs
                    if ref.source[-1] == "Module" and ref.phase == "escape"
                )
            if result.exposes_module_globals:
                _source_demands.append(
                    _SourceValueDemand(python_node_source_key(tree), "expose")
                )

    def _import_contexts(
        node: ast.AST, *, source_discovery: bool = False
    ) -> tuple[ModuleImportContext, ...]:
        return tuple(
            base_import_context.with_state(state)
            for state in (
                import_flow.source_states_for(node)
                if source_discovery
                else import_flow.states_for(node)
            )
        )

    source_bindings = _source_parameter_bindings(
        ast_digest_admission, source_owner, source_arguments
    )

    def _static_call_target(
        call: ast.Call, *, allow_possible: bool = False
    ) -> str | None:
        fact = binding_index.call_fact(call)
        if fact is not None:
            exact_kind = fact.exact_import_call_kind()
            if exact_kind == "dunder_import":
                return "builtins.__import__"
            if exact_kind == "import_module":
                return "importlib.import_module"
            if fact.callee_is(PythonIdentity.IMPORTLIB_FIND_SPEC):
                return "importlib.util.find_spec"
            if allow_possible:
                possible_kinds = fact.possible_import_call_kinds()
                if "dunder_import" in possible_kinds:
                    return "builtins.__import__"
                if "import_module" in possible_kinds:
                    return "importlib.import_module"
                if fact.callee_may_be(PythonIdentity.IMPORTLIB_FIND_SPEC):
                    return "importlib.util.find_spec"
        return None

    def _is_static_import_target(target: str | None) -> bool:
        return target in {
            "builtins.__import__",
            "importlib.import_module",
            "importlib.util.find_spec",
        }

    def _bound_static_value(
        node: ast.expr,
        bindings: Mapping[str, object],
    ) -> object | None:
        value = binding_index.static_value(node)
        if isinstance(value, PythonParameterRef):
            return bindings.get(value.name)
        return value

    def _bound_expression_result(
        node: ast.expr, bindings: Mapping[str, object]
    ) -> StaticExpressionResult:
        return _source_expression_result(binding_index, node, bindings)

    def _resolve_string_sequence(
        node: ast.expr, bindings: dict[str, object], seen: set[str]
    ) -> list[str] | None:
        value = _bound_static_value(node, bindings)
        if isinstance(value, tuple) and all(isinstance(item, str) for item in value):
            return [cast(str, item) for item in value]
        if isinstance(value, list) and all(isinstance(item, str) for item in value):
            return list(cast(list[str], value))
        return None

    def _resolve_string_constant(
        node: ast.expr,
        bindings: dict[str, object] | None = None,
        seen: set[str] | None = None,
    ) -> str | None:
        bindings = bindings or {}
        seen = seen or set()
        result = _source_expression_result(binding_index, node, bindings)
        value = (
            result.value if result.value_known else _bound_static_value(node, bindings)
        )
        if isinstance(value, str):
            return value
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = _resolve_string_constant(node.left, bindings, seen)
            right = _resolve_string_constant(node.right, bindings, seen)
            if left is not None and right is not None:
                return left + right
            return None
        if isinstance(node, ast.Call):
            if (
                isinstance(node.func, ast.Attribute)
                and node.func.attr == "join"
                and len(node.args) == 1
            ):
                sep = _resolve_string_constant(node.func.value, bindings, seen)
                if sep is None:
                    return None
                items = _resolve_string_sequence(node.args[0], bindings, seen)
                if items is None:
                    return None
                return sep.join(items)
            if isinstance(node.func, ast.Name):
                func_name = node.func.id
                if func_name in seen:
                    return None
                origins = binding_index.expression_result(node.func).source_origins
                if len(origins) != 1:
                    return None
                helper = ast_digest_admission.deferred_nodes.get(next(iter(origins)))
                if (
                    not isinstance(helper, (ast.FunctionDef, ast.AsyncFunctionDef))
                    or len(helper.body) != 1
                    or not isinstance(helper.body[0], ast.Return)
                    or helper.body[0].value is None
                    or helper.args.vararg is not None
                    or helper.args.kwarg is not None
                ):
                    return None
                params = [
                    arg.arg
                    for arg in (
                        *helper.args.posonlyargs,
                        *helper.args.args,
                        *helper.args.kwonlyargs,
                    )
                ]
                expr = helper.body[0].value
                if len(node.args) != len(params) or node.keywords:
                    return None
                child_bindings: dict[str, object] = dict(bindings)
                for param, arg in zip(params, node.args):
                    scalar = _resolve_string_constant(arg, bindings, seen)
                    if scalar is not None:
                        child_bindings[param] = scalar
                        continue
                    seq = _resolve_string_sequence(arg, bindings, seen)
                    if seq is not None:
                        child_bindings[param] = seq
                        continue
                    return None
                return _resolve_string_constant(
                    expr, child_bindings, seen | {func_name}
                )
        return None

    def _function_required_param_names(
        stmt: ast.FunctionDef | ast.AsyncFunctionDef, params: list[str]
    ) -> set[str]:
        positional = list(stmt.args.posonlyargs) + list(stmt.args.args)
        required_positional_count = max(0, len(positional) - len(stmt.args.defaults))
        required = {arg.arg for arg in positional[:required_positional_count]}
        for arg, default in zip(stmt.args.kwonlyargs, stmt.args.kw_defaults):
            if default is None:
                required.add(arg.arg)
        return required.intersection(params)

    def _simple_function_local_expr_bindings(
        stmt: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> dict[str, ast.expr]:
        values: dict[str, ast.expr] = {}
        repeated: set[str] = set()
        for node in ast.walk(stmt):
            assignment: tuple[ast.expr, ast.expr] | None = None
            if isinstance(node, ast.Assign) and len(node.targets) == 1:
                assignment = (node.targets[0], node.value)
            elif isinstance(node, ast.AnnAssign):
                if node.value is not None:
                    assignment = (node.target, node.value)
            if assignment is None:
                continue
            target, value = assignment
            if not isinstance(target, ast.Name):
                continue
            if target.id in values:
                repeated.add(target.id)
                continue
            values[target.id] = value
        for name in repeated:
            values.pop(name, None)
        return values

    def _resolve_local_expr_binding(
        expr: ast.expr, local_expr_bindings: dict[str, ast.expr]
    ) -> ast.expr:
        seen: set[str] = set()
        current = expr
        while isinstance(current, ast.Name) and current.id in local_expr_bindings:
            if current.id in seen:
                return expr
            seen.add(current.id)
            current = local_expr_bindings[current.id]
        return current

    def _static_import_call_payload(
        call: ast.Call,
        target: str,
        *,
        local_expr_bindings: Mapping[str, ast.expr] | None = None,
    ) -> _StaticImportCallPayload | None:
        operation_kind = (
            "import_module"
            if target in {"importlib.import_module", "importlib.util.find_spec"}
            else "dunder_import"
        )
        arguments = bind_static_import_call_arguments(call, operation_kind)
        if arguments is None:
            return None

        def resolve_local(expr: ast.expr | None) -> ast.expr | None:
            if expr is None or local_expr_bindings is None:
                return expr
            return _resolve_local_expr_binding(expr, dict(local_expr_bindings))

        name_expr = resolve_local(arguments.name)
        if target in {"importlib.import_module", "importlib.util.find_spec"}:
            return _StaticImportCallPayload(
                call=call,
                target=target,
                name=name_expr,
                package=resolve_local(arguments.package),
                requires_runtime_binding=arguments.requires_runtime_binding,
            )
        return _StaticImportCallPayload(
            call=call,
            target=target,
            name=name_expr,
            globals=resolve_local(arguments.globals),
            fromlist=resolve_local(arguments.fromlist),
            level=resolve_local(arguments.level),
            requires_runtime_binding=arguments.requires_runtime_binding,
        )

    def _resolve_static_import_call(
        payload: _StaticImportCallPayload,
        bindings: dict[str, object] | None = None,
    ) -> tuple[str, ...]:
        bindings = bindings or {}
        name = (
            _resolve_string_constant(payload.name, bindings, set())
            if payload.name is not None
            else None
        )
        source_requests: dict[
            StaticImportRequest,
            tuple[tuple[StaticImportRequest, ModuleImportContext], ...],
        ] = {}

        def source_requests_for(
            request: StaticImportRequest,
        ) -> tuple[tuple[StaticImportRequest, ModuleImportContext], ...]:
            if _dynamic_relative_import_discovery is None:
                return ()
            cached = source_requests.get(request)
            if cached is not None:
                return cached
            candidates = []
            for context in _import_contexts(payload.call, source_discovery=True):
                candidates.extend(
                    (candidate, context)
                    for candidate in source_import_requests_from_expressions(
                        request,
                        context,
                        package_expression=payload.package,
                        globals_expression=payload.globals,
                        source_contexts_for_read=lambda expression: _import_contexts(
                            expression, source_discovery=True
                        ),
                        resolve_string=resolve_string,
                        fact_result=lambda node: _bound_expression_result(
                            node, bindings
                        ),
                        expression_fact=binding_index.expression_fact,
                        call_fact=binding_index.call_fact(payload.call),
                    )
                )
            result = tuple(candidates)
            source_requests[request] = result
            return result

        def require_runtime_call_custody(
            reason: str,
            *,
            request: StaticImportRequest | None = None,
            context: ModuleImportContext | None = None,
            lexical_request: StaticImportRequest | None = None,
            star: bool = False,
        ) -> tuple[str, ...]:
            record_site(
                payload.call, request if star else None, (context,) if context else ()
            )
            if _dynamic_relative_import_discovery is not None:
                if request is None:
                    # Only unresolved binding or level loses the planner contract.
                    _dynamic_relative_import_discovery.record_call_binding(name)
                else:
                    assert context is not None
                    for source_request, source_context in source_requests_for(request):
                        _dynamic_relative_import_discovery.record(
                            source_request, (source_context,)
                        )
                    _dynamic_relative_import_discovery.record(
                        request, (context,), lexical_request=lexical_request
                    )
            if _runtime_catalog is not None:
                # The runtime may select any catalog row; completion appends it.
                if star and _star_modules is not None:
                    assert request is not None and context is not None
                    plan = plan_static_import_request(
                        replace(request, fromlist=()), (context,)
                    )
                    if plan.requires_runtime:
                        _runtime_catalog.star_modules = True
                    else:
                        _star_modules.extend(plan.modules)
                _runtime_catalog.imports = True
                return ()
            if _dynamic_relative_import_discovery is not None:
                if star and _dynamic_star_modules is not None:
                    assert request is not None and context is not None
                    owner_request = replace(request, fromlist=())
                    for source_request, source_context in source_requests_for(
                        owner_request
                    ):
                        _dynamic_star_modules.extend(
                            static_import_discovery(
                                source_request, (source_context,)
                            ).modules
                        )
                    _dynamic_star_modules.extend(
                        static_import_discovery(
                            owner_request,
                            (context,),
                            lexical_request=(
                                replace(lexical_request, fromlist=())
                                if lexical_request is not None
                                else None
                            ),
                        ).modules
                    )
                return ()
            raise UnresolvedStaticImportError(
                f"{reason} requires runtime import custody"
            )

        if payload.requires_runtime_binding:
            return require_runtime_call_custody("dynamic import argument expansion")
        if name is None:
            record_site(payload.call, None)
            return ()

        def resolve_string(expression: ast.expr) -> str | None:
            return _resolve_string_constant(expression, bindings, set())

        import_module_call = payload.target in {
            "importlib.import_module",
            "importlib.util.find_spec",
        }
        fromlist = (
            _resolve_string_sequence(payload.fromlist, bindings, set())
            if payload.fromlist is not None
            else []
        )
        if (
            fromlist is None
            and payload.fromlist is not None
            and static_import_fromlist_is_empty(
                _bound_expression_result(payload.fromlist, bindings)
            )
        ):
            # __import__ skips fromlist processing for every proven falsy value,
            # including explicit None. Unknown and expired values remain dynamic.
            fromlist = []
        level, level_is_invalid = (
            static_import_level_from_result(
                _bound_expression_result(payload.level, bindings)
            )
            if payload.level is not None
            else (0, False)
        )
        if (
            not import_module_call
            and payload.level is not None
            and level is None
            and not level_is_invalid
        ):
            return require_runtime_call_custody("non-literal __import__ level")
        contexts = _import_contexts(payload.call)
        modules: list[str] = []
        seen: set[str] = set()
        for context in contexts:
            lexical_context = ModuleImportContext(
                context.module_name,
                context.is_package,
                spec_name=context.spec_name,
                target_python=context.target_python,
                execution_kind=context.execution_kind,
            )
            if import_module_call:
                request = StaticImportRequest.import_module(
                    name,
                    metadata_value_from_expression(
                        payload.package,
                        context,
                        resolve_string,
                        fact_result=lambda node: _bound_expression_result(
                            node, bindings
                        ),
                        expression_fact=binding_index.expression_fact,
                        call_fact=binding_index.call_fact(payload.call),
                    ),
                )
                lexical_request = StaticImportRequest.import_module(
                    name,
                    metadata_value_from_expression(
                        payload.package,
                        lexical_context,
                        resolve_string,
                        fact_result=lambda node: _bound_expression_result(
                            node, bindings
                        ),
                        expression_fact=binding_index.expression_fact,
                        call_fact=binding_index.call_fact(payload.call),
                        allow_activation_metadata_for_discovery=True,
                    ),
                )
            else:
                # Establish the base and lexical twin before dynamic fromlist
                # admission. Empty fromlist preserves name/level/globals while
                # leaving child selection to runtime custody.
                request = StaticImportRequest(
                    "dunder_import",
                    name,
                    level=0 if level is None else level,
                    level_is_invalid=level_is_invalid,
                    globals_state=dunder_globals_state_from_expression(
                        payload.globals,
                        context,
                        resolve_string,
                        fact_result=lambda node: _bound_expression_result(
                            node, bindings
                        ),
                        expression_fact=binding_index.expression_fact,
                        call_fact=binding_index.call_fact(payload.call),
                    ),
                    globals_were_supplied=payload.globals is not None,
                )
                lexical_request = StaticImportRequest(
                    "dunder_import",
                    name,
                    level=0 if level is None else level,
                    level_is_invalid=level_is_invalid,
                    globals_state=dunder_globals_state_from_expression(
                        payload.globals,
                        lexical_context,
                        resolve_string,
                        fact_result=lambda node: _bound_expression_result(
                            node, bindings
                        ),
                        expression_fact=binding_index.expression_fact,
                        call_fact=binding_index.call_fact(payload.call),
                        allow_possible_current_globals=True,
                    ),
                    globals_were_supplied=payload.globals is not None,
                )
                star = fromlist is not None and "*" in fromlist
                if fromlist is None or star:
                    if star and _dynamic_relative_import_discovery is not None:
                        # Explicit siblings in ('child', '*') are candidates
                        # too; only the base participates in __all__ expansion.
                        assert fromlist is not None
                        children = tuple(item for item in fromlist if item != "*")
                        if children:
                            for source_request, source_context in source_requests_for(
                                replace(request, fromlist=children)
                            ):
                                _dynamic_relative_import_discovery.record(
                                    source_request, (source_context,)
                                )
                            _dynamic_relative_import_discovery.record(
                                replace(request, fromlist=children),
                                (context,),
                                lexical_request=replace(
                                    lexical_request, fromlist=children
                                ),
                            )
                    resolved = require_runtime_call_custody(
                        "dynamic __import__ star fromlist"
                        if star
                        else "non-literal __import__ fromlist",
                        request=replace(request, fromlist=tuple(fromlist))
                        if star
                        else request,
                        context=context,
                        lexical_request=lexical_request,
                        star=star,
                    )
                    for module in resolved:
                        if module not in seen:
                            seen.add(module)
                            modules.append(module)
                    continue
                request = replace(request, fromlist=tuple(fromlist))
                lexical_request = replace(lexical_request, fromlist=tuple(fromlist))
            record_site(payload.call, request, (context,))
            sealed = _sealed_import_modules(
                request,
                (context,),
                runtime_catalog_owner=_runtime_catalog is not None,
                dynamic_relative_import_discovery=_dynamic_relative_import_discovery,
                lexical_discovery_request=lexical_request,
                source_discovery_requests=source_requests_for(request),
            )
            if sealed.runtime_catalog:
                assert _runtime_catalog is not None
                _runtime_catalog.imports = True
            for module in sealed.modules:
                if module not in seen:
                    seen.add(module)
                    modules.append(module)
        return tuple(modules)

    def _bind_helper_call_arguments(
        call: ast.Call, params: list[str], required_params: set[str]
    ) -> dict[str, object] | None:
        if len(call.args) > len(params):
            return None
        bindings: dict[str, object] = {}
        for idx, arg in enumerate(call.args):
            param = params[idx]
            scalar = _resolve_string_constant(arg)
            if scalar is not None:
                bindings[param] = scalar
                continue
            result = _bound_expression_result(arg, {})
            if result.value_known:
                bindings[param] = result.value
                continue
            seq = _resolve_string_sequence(arg, {}, set())
            if seq is not None:
                bindings[param] = seq
        for keyword in call.keywords:
            if keyword.arg is None or keyword.arg not in params:
                return None
            if keyword.arg in bindings:
                return None
            scalar = _resolve_string_constant(keyword.value)
            if scalar is not None:
                bindings[keyword.arg] = scalar
                continue
            result = _bound_expression_result(keyword.value, {})
            if result.value_known:
                bindings[keyword.arg] = result.value
                continue
            seq = _resolve_string_sequence(keyword.value, {}, set())
            if seq is not None:
                bindings[keyword.arg] = seq
        if not required_params.issubset(bindings):
            return None
        return bindings

    def _source_helper_imports(stmt):
        params = [
            arg.arg
            for arg in (
                list(stmt.args.posonlyargs)
                + list(stmt.args.args)
                + list(stmt.args.kwonlyargs)
            )
        ]
        if stmt.args.vararg is not None:
            params.append(stmt.args.vararg.arg)
        if stmt.args.kwarg is not None:
            params.append(stmt.args.kwarg.arg)
        if not params:
            return None
        required_params = _function_required_param_names(stmt, params)
        local_expr_bindings = _simple_function_local_expr_bindings(stmt)
        for node in ast.walk(stmt):
            if not isinstance(node, ast.Call):
                continue
            if not import_flow.states_for(node):
                continue
            target = _static_call_target(node, allow_possible=True)
            if not _is_static_import_target(target):
                continue
            assert target is not None
            payload = _static_import_call_payload(
                node,
                target,
                local_expr_bindings=local_expr_bindings,
            )
            if payload is None:
                continue
            helper_entry = helper_import_calls.get(python_node_source_key(stmt))
            if helper_entry is None:
                helper_import_calls[python_node_source_key(stmt)] = (
                    params,
                    required_params,
                    [payload],
                )
            else:
                helper_entry[2].append(payload)

        return helper_import_calls.get(python_node_source_key(stmt))

    scanned_helpers: set[PythonSourceKey] = set()

    def _record_helper_call_imports(node: ast.Call) -> None:
        nonlocal projecting_helper
        for origin in binding_index.expression_result(node.func).source_origins:
            stmt = ast_digest_admission.deferred_nodes.get(origin)
            if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            if origin not in scanned_helpers:
                scanned_helpers.add(origin)
                _source_helper_imports(stmt)
            helper_call_entry = helper_import_calls.get(origin)
            if helper_call_entry is not None:
                params, required_params, payloads = helper_call_entry
                call_bindings = _bind_helper_call_arguments(
                    node, params, required_params
                )
                if call_bindings is not None:
                    projecting_helper = True
                    try:
                        for payload in payloads:
                            imports.extend(
                                _resolve_static_import_call(payload, call_bindings)
                            )
                    finally:
                        projecting_helper = False

    def _record_import_statement(
        node: ast.Import | ast.ImportFrom,
    ) -> None:
        if isinstance(node, ast.Import):
            for alias in node.names:
                record_site(
                    node,
                    StaticImportRequest.statement(alias.name),
                    _import_contexts(node),
                )
                imports.append(alias.name)
                if _source_bindings is not None:
                    _source_bindings.append(
                        _SourceImportBinding(
                            python_node_source_key(alias),
                            (
                                alias.name
                                if alias.asname
                                else alias.name.split(".", 1)[0],
                            ),
                            None,
                        )
                    )
            return
        names = tuple(python_source_field(alias, "name") for alias in node.names)
        if node.level == 0:
            record_site(
                node,
                StaticImportRequest.statement(node.module or "", fromlist=names),
                _import_contexts(node),
            )
            imports.extend(static_import_candidates(node.module or "", names))
            if _source_bindings is not None:
                _source_bindings.extend(
                    _SourceImportBinding(
                        python_node_source_key(alias),
                        (node.module or "",),
                        python_source_field(alias, "name"),
                    )
                    for alias in node.names
                )
            if _star_modules is not None and "*" in names and node.module:
                _star_modules.append(node.module)
            return
        request = StaticImportRequest.statement(
            node.module or "", level=node.level, fromlist=names
        )
        statement_fact = binding_index.statement_fact(node)
        lexical_contexts = (
            _import_contexts(node, source_discovery=True)
            if _dynamic_relative_import_discovery is not None
            else ()
        )
        contexts = tuple(
            module_import_context_with_metadata_proof(
                context,
                statement_fact.module_metadata_at_entry
                if statement_fact is not None
                else None,
            )
            for context in _import_contexts(node)
        )
        record_site(node, request, contexts)
        sealed = _sealed_import_modules(
            request,
            contexts,
            lexical_discovery_contexts=lexical_contexts,
            runtime_catalog_owner=_runtime_catalog is not None,
            dynamic_relative_import_discovery=_dynamic_relative_import_discovery,
        )
        resolved = sealed.modules
        imports.extend(resolved)
        if sealed.runtime_catalog:
            assert _runtime_catalog is not None
            _runtime_catalog.imports = True
            if "*" in names:
                _runtime_catalog.star_modules = True
        if _source_bindings is not None:
            # Resolve the base with the identical source-ordered context, rather
            # than stripping a member suffix from an ambiguous candidate list.
            base_request = StaticImportRequest.statement(
                node.module or "", level=node.level, fromlist=()
            )
            sealed_bases = _sealed_import_modules(
                base_request,
                contexts,
                lexical_discovery_contexts=lexical_contexts,
                runtime_catalog_owner=_runtime_catalog is not None,
                dynamic_relative_import_discovery=_dynamic_relative_import_discovery,
            )
            bases = sealed_bases.modules
            if _dynamic_relative_import_discovery is not None:
                bases = tuple(
                    dict.fromkeys(
                        (
                            *bases,
                            *static_import_discovery(
                                base_request, lexical_contexts
                            ).modules,
                        )
                    )
                )
            _source_bindings.extend(
                _SourceImportBinding(
                    python_node_source_key(alias),
                    bases,
                    python_source_field(alias, "name"),
                    sealed_bases.runtime_catalog,
                )
                for alias in node.names
            )
        if _star_modules is not None and "*" in names:
            _star_modules.extend(resolved)

    def _collect_import_call(node: ast.Call) -> None:
        if not import_flow.states_for(node):
            return
        _record_helper_call_imports(node)
        # Graph custody includes every admitted import alternative after a
        # callback; exact callee identity is a lowering/specialization condition.
        target = _static_call_target(node, allow_possible=True)
        if not _is_static_import_target(target):
            return
        assert target is not None
        payload = _static_import_call_payload(node, target)
        if payload is None:
            record_site(node, None)
        else:
            modules = _resolve_static_import_call(payload, source_bindings)
            imports.extend(modules)
            if modules and _source_bindings is not None:
                # The returned module is one of those the call imports; uses of
                # the result select from them, as uses of an import alias do.
                _source_bindings.append(
                    _SourceImportBinding(
                        python_node_source_key(node),
                        tuple(dict.fromkeys(modules)),
                        None,
                    )
                )

    def publishes_binding(node: ast.AST, name: str) -> bool:
        fact = (
            binding_index.statement_fact(node)
            if isinstance(node, ast.stmt)
            else binding_index.expression_fact(node)
            if isinstance(node, ast.expr)
            else None
        )
        if fact is None:
            return False
        scope = binding_index.scopes[fact.scope_id]
        return name in scope.global_names or name in scope.nonlocal_names

    def publishes_target(node: ast.stmt, target: ast.AST) -> bool:
        if isinstance(target, ast.Name):
            return publishes_binding(node, target.id)
        if isinstance(target, (ast.Tuple, ast.List)):
            return any(publishes_target(node, child) for child in target.elts)
        if isinstance(target, ast.Starred):
            return publishes_target(node, target.value)
        return isinstance(target, (ast.Attribute, ast.Subscript))

    def demand_iteration(node: ast.AST) -> None:
        iteration = binding_index.iteration_fact(node)
        if iteration is not None and _source_demands is not None:
            _source_demands.extend(
                _SourceValueDemand(ref.source, ref.phase)
                for ref in sorted(
                    iteration.deferred_execution,
                    key=lambda ref: (ref.source, ref.phase),
                )
            )

    def _visit(
        node: ast.AST,
    ) -> None:
        nonlocal needs_string_templatelib, needs_typing
        if isinstance(node, (ast.stmt, ast.Call)) and not import_flow.states_for(node):
            return
        fact = (
            binding_index.statement_fact(node)
            if isinstance(node, ast.stmt)
            else binding_index.expression_fact(node)
            if isinstance(node, ast.expr)
            else None
        )
        if fact is not None and _source_demands is not None:
            _source_demands.extend(
                _SourceValueDemand(ref.source, ref.phase)
                for ref in sorted(
                    fact.deferred_execution, key=lambda ref: (ref.source, ref.phase)
                )
            )
        if isinstance(node, (ast.For, ast.AsyncFor)):
            demand_iteration(node)
        elif isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp)):
            for generator in node.generators:
                demand_iteration(generator)
        if isinstance(node, ast.Call):
            demand(binding_index.expression_result(node.func), "call")
            for value in (*node.args, *(keyword.value for keyword in node.keywords)):
                demand(
                    binding_index.expression_result(
                        value.value if isinstance(value, ast.Starred) else value
                    ),
                    "escape",
                    contents=True,
                )
        elif isinstance(node, ast.Attribute) and isinstance(node.ctx, ast.Load):
            demand(binding_index.expression_result(node), "read")
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if isinstance(node, ast.ClassDef):
                for base in node.bases:
                    # Loading a base is not constructing an instance of it.
                    # Calls in the expression and class preparation retain the
                    # binding evaluator's own execution facts.
                    demand(
                        binding_index.expression_result(
                            base.value if isinstance(base, ast.Starred) else base
                        ),
                        "escape",
                        contents=isinstance(base, ast.Starred),
                    )
                for keyword in node.keywords:
                    demand(
                        binding_index.expression_result(keyword.value),
                        "escape",
                        contents=True,
                    )
            for decorator in node.decorator_list:
                demand(binding_index.expression_result(decorator), "call")
            if (
                node.decorator_list or publishes_binding(node, node.name)
            ) and _source_demands is not None:
                _source_demands.append(
                    _SourceValueDemand(python_node_source_key(node), "expose")
                )
        elif isinstance(node, ast.Assign) and any(
            publishes_target(node, target) for target in node.targets
        ):
            demand(binding_index.expression_result(node.value), "escape", contents=True)
        elif (
            isinstance(node, (ast.AnnAssign, ast.AugAssign))
            and node.value is not None
            and publishes_target(node, node.target)
        ):
            demand(binding_index.expression_result(node.value), "escape", contents=True)
        elif (
            isinstance(node, ast.NamedExpr)
            and isinstance(node.target, ast.Name)
            and publishes_binding(node, node.target.id)
        ):
            demand(binding_index.expression_result(node.value), "escape", contents=True)
        elif isinstance(node, (ast.For, ast.AsyncFor)) and publishes_target(
            node, node.target
        ):
            demand(binding_index.expression_result(node.iter), "escape", contents=True)
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                if item.optional_vars is not None and publishes_target(
                    node, item.optional_vars
                ):
                    demand(
                        binding_index.expression_result(item.context_expr),
                        "escape",
                        contents=True,
                    )
        elif isinstance(node, ast.Match) and any(
            publishes_binding(node, name)
            for case in node.cases
            for name in python_pattern_capture_names(case.pattern)
        ):
            demand(
                binding_index.expression_result(node.subject), "escape", contents=True
            )
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            _record_import_statement(node)
            if isinstance(node, ast.ImportFrom) and _source_demands is not None:
                _source_demands.extend(
                    _SourceValueDemand(
                        python_node_source_key(alias),
                        "expose"
                        if publishes_binding(node, alias.asname or alias.name)
                        else "read",
                    )
                    for alias in node.names
                )
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            needs_typing |= bool(getattr(node, "type_params", ()))
        elif isinstance(node, ast.TypeAlias):
            needs_typing = True
            if publishes_target(node, node.name) and _source_demands is not None:
                _source_demands.append(
                    _SourceValueDemand(python_node_source_key(node), "expose")
                )
        elif template_str_cls is not None and isinstance(node, template_str_cls):
            needs_string_templatelib = True
        elif isinstance(node, ast.Call):
            _collect_import_call(node)

    if source_owner is not None:
        owner_node = ast_digest_admission.deferred_nodes.get(source_owner)
        if isinstance(owner_node, ast.GeneratorExp):
            # The comprehension wrappers are not expression execution regions.
            # Their already-derived protocol facts belong to advancement of
            # this generator body, never to creation of the generator value.
            for generator in owner_node.generators:
                demand_iteration(generator)
    for region in python_execution_regions(
        cast(ast.Module, tree),
        target_python=target_python.feature_version,
        include_deferred=import_scan_mode == "full" and source_owner is None,
        source_owner=source_owner,
        body=_source_body(tree, source_owner, ast_digest_admission),
        fact_result=lambda expression: _source_expression_result(
            binding_index, expression, source_bindings
        ),
    ):
        if region.phase == "evaluate":
            _visit(region.node)
        else:
            assert isinstance(region.node, ast.expr)
            demand(binding_index.expression_result(region.node), region.phase)
    if needs_typing:
        imports.append("typing")
    if needs_string_templatelib:
        imports.append("string.templatelib")
    return imports


def _collect_imports_for_graph(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    import_scan_mode: ImportScanMode = "full",
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_catalog_owner: bool = False,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> _ImportDiscoveryProjection:
    """Scan one source's graph requests. A runtime-import owner's scan defers
    its dynamic-import sites to the runtime catalog instead of expanding it, so
    the result depends on the source alone."""
    ast_digest_admission = _PythonAstDigestAdmission.for_tree(
        tree, ast_digest_admission
    )
    runtime_catalog = _RuntimeCatalogUse() if runtime_catalog_owner else None
    discovery = _DynamicRelativeImportDiscovery()
    star_modules: list[str] = []
    dynamic_star_modules: list[str] = []
    source_demands: list[_SourceValueDemand] = []
    source_bindings: list[_SourceImportBinding] = []
    source_values: list[_SourceValueSummary] = []
    source_sites: list[SourceImportSite] = []
    runtime_import_protocol: list[bool] = []
    imports = _collect_imports(
        tree,
        module_name,
        is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=ast_digest_admission,
        source_owner=source_owner,
        source_arguments=source_arguments,
        _runtime_catalog=runtime_catalog,
        _source_demands=source_demands,
        _source_bindings=source_bindings,
        _source_values=source_values,
        _source_sites=source_sites,
        _dynamic_relative_import_discovery=discovery,
        _star_modules=star_modules,
        _dynamic_star_modules=dynamic_star_modules,
        _runtime_import_protocol=runtime_import_protocol,
    )
    return discovery.projection(imports, star_modules, dynamic_star_modules)._replace(
        uses_runtime_import_protocol=runtime_import_protocol[0],
        source_demands=tuple(dict.fromkeys(source_demands)),
        source_bindings=tuple(dict.fromkeys(source_bindings)),
        source_values=source_values[0] if source_values else _SourceValueSummary(),
        source_sites=tuple(dict.fromkeys(source_sites)),
        all_exports=_static_module_all_exports(
            tree,
            target_python=target_python,
            namespace_premise=namespace_premise,
            ast_digest_admission=ast_digest_admission,
        ),
        namespace_summary=(
            python_namespace_summary(
                tree,
                package=python_package_name(module_name or "", is_package=is_package),
                target_python=target_python.feature_version,
            )
            if source_owner is None and isinstance(tree, ast.Module)
            else None
        ),
        runtime_catalog=(
            None
            if runtime_catalog is None
            else runtime_catalog.deferral(ast_digest_admission.digest)
        ),
    )


def _static_string_sequence(node: ast.expr) -> tuple[str, ...] | None:
    if not isinstance(node, (ast.Tuple, ast.List)):
        return None
    out: list[str] = []
    for item in node.elts:
        if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
            return None
        out.append(item.value)
    return tuple(out)


def _static_module_all_exports(
    tree: ast.AST,
    *,
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> SourceAllExports:
    """One module export fact per admitted AST, target and namespace premise.

    Body-owner and argument projections share this module-level fact. Its
    storage belongs to the read-only source generation, never a global cache.
    """
    admission = _PythonAstDigestAdmission.for_tree(tree, ast_digest_admission)
    context = (target_python, namespace_premise)
    cached = admission.module_all_exports.get(context)
    if cached is None:
        cached = _compute_static_module_all_exports(
            tree,
            target_python=target_python,
            namespace_premise=namespace_premise,
            ast_digest_admission=admission,
        )
        admission.module_all_exports[context] = cached
    return cached


def _compute_static_module_all_exports(
    tree: ast.AST,
    *,
    namespace_premise: PythonNamespacePremise | None,
    target_python: TargetPythonVersion,
    ast_digest_admission: _PythonAstDigestAdmission,
) -> SourceAllExports:
    """Prove a literal export set only while its source owner stays sealed.

    The existing binding authority decides whether executed code can observe
    the module namespace. This is not an evaluator for arbitrary __all__ code.
    An unknown result remains an import obligation, never an empty export set.
    """
    if not isinstance(tree, ast.Module):
        return SourceAllExports("incomplete", reason="export source is not a module")
    if namespace_premise is not None and (
        namespace_premise.foreign_writes is None
        or "__all__" in namespace_premise.foreign_writes
    ):
        return SourceAllExports(
            "unknown", reason="graph code may rebind the export namespace"
        )
    admission = _PythonAstDigestAdmission.for_tree(tree, ast_digest_admission)
    facts = analyze_python_binding_facts(
        tree,
        source_digest=admission.digest,
        ast_digest_admission=admission,
        policy=_source_binding_policy(
            target_python=target_python, namespace_premise=namespace_premise
        ).flow_policy(),
    )
    _future, eager = module_annotation_execution(
        tree, target_python=target_python.feature_version
    )
    exports: tuple[str, ...] | None = None
    unknown: str | None = None
    for stmt in tree.body:
        direct = (
            isinstance(stmt, ast.Assign)
            and len(stmt.targets) == 1
            and isinstance(stmt.targets[0], ast.Name)
            and stmt.targets[0].id == "__all__"
        ) or (
            isinstance(stmt, ast.AnnAssign)
            and isinstance(stmt.target, ast.Name)
            and stmt.target.id == "__all__"
            and stmt.value is not None
        )
        if (
            direct
            and isinstance(stmt, (ast.Assign, ast.AnnAssign))
            and stmt.value is not None
        ):
            sequence = _static_string_sequence(stmt.value)
            if sequence is not None and not facts.module_namespace_may_be_observed(
                stmt
            ):
                exports, unknown = sequence, None
                continue
            unknown = "nonliteral or observable __all__ assignment"
        elif (
            "__all__"
            in python_scope_declarations([stmt], eager_annotations=eager).bound
        ):
            unknown = "nonliteral or conditional __all__ binding"
        elif facts.module_namespace_may_be_observed(stmt):
            unknown = "executed code may change the module export namespace"
        else:
            # Passing/aliasing the mutable declaration or writing a subscript
            # invalidates this literal proof. Deferred bodies are deliberately
            # absent; invoked bodies expose their effects through binding facts.
            if any(
                region.phase == "evaluate"
                and isinstance(region.node, ast.Name)
                and region.node.id == "__all__"
                and isinstance(region.node.ctx, (ast.Load, ast.Del))
                for region in python_execution_regions(
                    ast.Module(body=[stmt], type_ignores=[]),
                    target_python=target_python.feature_version,
                )
            ):
                unknown = "__all__ is exposed or mutated"
    if unknown is None and exports is None:
        # Absence is a lookup claim. PEP 562 can supply __all__ even when no
        # direct binding exists; source hooks are never an empty export set.
        if any(
            name == "__getattr__"
            for owner, names, _bases in facts.source_namespaces
            if owner[-1] == "Module"
            for name, _origins in names
        ):
            unknown = "module __getattr__ may supply __all__"
    if unknown is not None:
        return SourceAllExports("unknown", reason=unknown)
    return (
        SourceAllExports("absent")
        if exports is None
        else SourceAllExports("known", exports)
    )


def _collect_import_star_modules(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    _dynamic_relative_import_discovery: _DynamicRelativeImportDiscovery | None = None,
    _dynamic_star_modules: list[str] | None = None,
) -> tuple[str, ...]:
    # Use the same binding facts, helper forwarding, request planner and scan
    # depth as ordinary imports. Star collection is a projection, not an analyzer.
    out: list[str] = []
    _collect_imports(
        tree,
        module_name,
        is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        namespace_premise=namespace_premise,
        ast_digest_admission=ast_digest_admission,
        _dynamic_relative_import_discovery=_dynamic_relative_import_discovery,
        _star_modules=out,
        _dynamic_star_modules=_dynamic_star_modules,
    )
    return tuple(dict.fromkeys(out))


def _collect_import_scan_requests(
    projection: _ImportDiscoveryProjection,
    tree: ast.AST,
    *,
    source_path: Path,
    module_name: str,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    import_scan_mode: ImportScanMode,
    namespace_premise: PythonNamespacePremise | None = None,
    target_python: TargetPythonVersion,
    ast_digest_admission: _PythonAstDigestAdmission,
    source: str | None = None,
) -> _ImportScanRequests:
    executions = (
        ()
        if source is not None and not _source_may_use_static_source_execution(source)
        else _collect_static_source_execution_requests(
            tree,
            source_path=source_path,
            import_scan_mode=import_scan_mode,
            source_owner=source_owner,
            source_arguments=source_arguments,
            module_name=module_name,
            target_python=target_python,
            namespace_premise=namespace_premise,
            ast_digest_admission=ast_digest_admission,
        )
    )
    return _ImportScanRequests(
        projection.imports,
        executions,
        projection.star_modules,
        projection.dynamic_relative_import_candidates,
        projection.requires_runtime_package_anchor,
        projection.dynamic_star_modules,
        projection.source_demands,
        projection.source_bindings,
        projection.source_values,
        projection.source_sites,
        projection.all_exports,
        projection.namespace_summary,
        projection.runtime_catalog,
        uses_runtime_import_protocol=projection.uses_runtime_import_protocol,
    )
