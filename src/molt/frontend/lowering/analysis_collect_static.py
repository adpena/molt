"""AnalysisCollectStaticMixin: frontend collection and static-fact helpers.

Move-only extraction from frontend/__init__.py. Owns symbol/name collection,
free-variable discovery, module-scope prewalk facts, comprehension capture facts,
and static truthiness helpers. Pattern recognizers live in analysis_patterns.py.
"""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
    Iterable,
    Sequence,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
    _canonical_intrinsic_runtime_name,
)
from molt.compiler_analysis.python_source_keys import python_pattern_capture_names
from molt.compiler_analysis.python_lexical_scope import (
    PythonDependencyAuthority,
    LexicalDefinitionNode,
    PythonLexicalScopeVisitor,
    ScopedNamedExprCollector,
)
from molt.compiler_analysis.static_truth import (
    static_expression_result,
    StaticTruthKwargs,
    static_if_live_branch,
)

if TYPE_CHECKING:
    from molt.compiler_analysis.python_binding_facts import PythonBindingIndex
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AnalysisCollectStaticMixin(_MixinBase):
    _lexical_dependency_cache: PythonDependencyAuthority | None = None
    _lexical_dependency_index: PythonBindingIndex | None = None

    def _static_truth_kwargs(self) -> StaticTruthKwargs:
        index = self.python_binding_index
        return {} if index is None else {"fact_result": index.expression_result}

    def _collect_module_annotation_items(
        self, node: ast.Module
    ) -> tuple[list[tuple[str, ast.expr, int]], dict[int, int]]:
        items: list[tuple[str, ast.expr, int]] = []
        id_map: dict[int, int] = {}
        outer = self

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_If(self, node: ast.If) -> None:
                # CPython does not record annotations from a statically-dead
                # branch (`if False:`/`if TYPE_CHECKING:`) in `__annotations__`.
                static_branch = static_if_live_branch(
                    node,
                    **outer._static_truth_kwargs(),
                )
                if static_branch is not None:
                    if static_expression_result(
                        node.test, **outer._static_truth_kwargs()
                    ).evaluation_required:
                        self.visit(node.test)
                    for stmt in static_branch:
                        self.visit(stmt)
                    return None
                self.generic_visit(node)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    exec_id = len(items)
                    items.append((node.target.id, node.annotation, exec_id))
                    id_map[id(node)] = exec_id

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return items, id_map

    def _collect_global_rebinds(self, node: ast.AST) -> set[str]:
        names: set[str] = set()
        for current in ast.walk(node):
            if isinstance(current, ast.Global):
                names.update(current.names)
        return names

    def _collect_module_assignments(
        self, node: ast.Module
    ) -> tuple[dict[str, int], set[str], bool]:
        counts: dict[str, int] = {}
        func_defs: set[str] = set()
        has_dynamic_bind = False
        outer = self

        def record(name: str) -> None:
            counts[name] = counts.get(name, 0) + 1

        def record_target(target: ast.AST) -> None:
            if isinstance(target, ast.Name):
                record(target.id)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt)
            elif isinstance(target, ast.Starred):
                record_target(target.value)
            else:
                scoped_writes.visit(target)

        def record_pattern(pattern: ast.pattern) -> None:
            for name in python_pattern_capture_names(pattern):
                record(name)

        scoped_writes = ScopedNamedExprCollector(
            record,
            eager_annotations=self.eager_annotations and not self.future_annotations,
        )

        class Collector(PythonLexicalScopeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                scoped_writes.visit(node)
                return None

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                scoped_writes.visit(node)
                return None

            def visit_ClassDef(self, node: ast.ClassDef) -> Any:
                record(node.name)
                scoped_writes.visit(node)
                return None

            def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
                record_target(node.name)

            def visit_Lambda(self, node: ast.Lambda) -> Any:
                scoped_writes.visit(node)
                return None

            def visit_ListComp(self, node: ast.ListComp) -> Any:
                scoped_writes.visit(node)
                return None

            def visit_SetComp(self, node: ast.SetComp) -> Any:
                scoped_writes.visit(node)
                return None

            def visit_DictComp(self, node: ast.DictComp) -> Any:
                scoped_writes.visit(node)
                return None

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
                scoped_writes.visit(node)
                return None

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    record_target(node.target)
                super().visit_AnnAssign(node)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                record_target(node.target)
                self.visit(node.iter)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                record_target(node.target)
                self.visit(node.iter)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_While(self, node: ast.While) -> None:
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_If(self, node: ast.If) -> None:
                static_branch = static_if_live_branch(
                    node,
                    **outer._static_truth_kwargs(),
                )
                if static_branch is not None:
                    if static_expression_result(
                        node.test, **outer._static_truth_kwargs()
                    ).evaluation_required:
                        self.visit(node.test)
                    for stmt in static_branch:
                        self.visit(stmt)
                    return None
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    self.visit(item.context_expr)
                    if item.optional_vars is not None:
                        record_target(item.optional_vars)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    self.visit(item.context_expr)
                    if item.optional_vars is not None:
                        record_target(item.optional_vars)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_Try(self, node: ast.Try) -> None:
                for stmt in node.body:
                    self.visit(stmt)
                for handler in node.handlers:
                    self.visit(handler)
                for stmt in node.orelse:
                    self.visit(stmt)
                for stmt in node.finalbody:
                    self.visit(stmt)

            def visit_TryStar(self, node: ast.TryStar) -> None:
                for stmt in node.body:
                    self.visit(stmt)
                for handler in node.handlers:
                    self.visit(handler)
                for stmt in node.orelse:
                    self.visit(stmt)
                for stmt in node.finalbody:
                    self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    record(node.name)
                if node.type is not None:
                    self.visit(node.type)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    record_pattern(case.pattern)
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_Import(self, node: ast.Import) -> None:
                for alias in node.names:
                    name = alias.asname or alias.name.split(".", 1)[0]
                    record(name)

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                nonlocal has_dynamic_bind
                for alias in node.names:
                    if alias.name == "*":
                        has_dynamic_bind = True
                        continue
                    name = alias.asname or alias.name
                    record(name)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target)

        collector = Collector(eager_annotations=scoped_writes.eager_annotations)
        for stmt in node.body:
            collector.visit(stmt)
        return counts, func_defs, has_dynamic_bind

    def _collect_module_class_mutations(self, node: ast.Module) -> set[str]:
        class_names = {
            stmt.name for stmt in node.body if isinstance(stmt, ast.ClassDef)
        }
        if not class_names:
            return set()
        mutated: set[str] = set()

        def record_target(target: ast.AST) -> None:
            if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name):
                if target.value.id in class_names:
                    mutated.add(target.value.id)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt)
            elif isinstance(target, ast.Starred):
                record_target(target.value)

        class Collector(ast.NodeVisitor):
            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target)

            def visit_Call(self, node: ast.Call) -> None:
                if (
                    isinstance(node.func, ast.Name)
                    and node.func.id in {"setattr", "delattr"}
                    and node.args
                ):
                    target = node.args[0]
                    if isinstance(target, ast.Name) and target.id in class_names:
                        mutated.add(target.id)
                self.generic_visit(node)

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return mutated

    def _collect_module_optional_intrinsic_globals(
        self, node: ast.Module
    ) -> dict[str, str]:
        bindings: dict[str, str] = {}

        def clear_name(name: str) -> None:
            bindings.pop(name, None)

        def assigned_names(target: ast.AST) -> list[str]:
            if isinstance(target, ast.Name):
                return [target.id]
            if isinstance(target, (ast.Tuple, ast.List)):
                names: list[str] = []
                for elt in target.elts:
                    names.extend(assigned_names(elt))
                return names
            return []

        for stmt in node.body:
            if isinstance(stmt, ast.Assign):
                runtime_name = self._match_optional_intrinsic_loader_expr(stmt.value)
                for target in stmt.targets:
                    for name in assigned_names(target):
                        if runtime_name is None:
                            clear_name(name)
                        else:
                            bindings[name] = _canonical_intrinsic_runtime_name(
                                runtime_name
                            )
                continue
            if isinstance(stmt, ast.AnnAssign):
                for name in assigned_names(stmt.target):
                    if stmt.value is None:
                        continue
                    runtime_name = self._match_optional_intrinsic_loader_expr(
                        stmt.value
                    )
                    if runtime_name is None:
                        clear_name(name)
                    else:
                        bindings[name] = _canonical_intrinsic_runtime_name(runtime_name)
                continue
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                clear_name(stmt.name)
                continue
            if isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    clear_name(alias.asname or alias.name.split(".")[0])
                continue
            if isinstance(stmt, ast.ImportFrom):
                for alias in stmt.names:
                    if alias.name != "*":
                        clear_name(alias.asname or alias.name)
                continue
            if isinstance(stmt, (ast.For, ast.AsyncFor)):
                for name in assigned_names(stmt.target):
                    clear_name(name)
                continue
            if isinstance(stmt, ast.With):
                for item in stmt.items:
                    if item.optional_vars is not None:
                        for name in assigned_names(item.optional_vars):
                            clear_name(name)
                continue
        return bindings

    def _collect_pattern_capture_names(self, pattern: ast.pattern) -> list[str]:
        return list(python_pattern_capture_names(pattern))

    def _collect_assigned_names(self, nodes: list[ast.stmt]) -> set[str]:
        return set(self._collect_assigned_names_ordered(nodes))

    def _collect_assigned_names_ordered(self, nodes: list[ast.stmt]) -> list[str]:
        outer = self

        class AssignCollector(PythonLexicalScopeVisitor):
            def __init__(self) -> None:
                super().__init__(
                    eager_annotations=outer.eager_annotations
                    and not outer.future_annotations
                )
                self.names: list[str] = []
                self.seen: set[str] = set()

            def _add(self, name: str) -> None:
                if name not in self.seen:
                    self.seen.add(name)
                    self.names.append(name)

            def _add_targets(self, target: ast.AST) -> None:
                if isinstance(target, ast.Name):
                    self._add(target.id)
                elif isinstance(target, (ast.Tuple, ast.List)):
                    for element in target.elts:
                        self._add_targets(element)
                elif isinstance(target, ast.Starred):
                    self._add_targets(target.value)
                else:
                    for name in outer._collect_namedexpr_names(target):
                        self._add(name)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self._add_targets(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    self._add_targets(node.target)
                super().visit_AnnAssign(node)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self._add_targets(node.target)
                self.visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self._add_targets(node.target)
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self._add_targets(node.target)
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._add_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._add_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_If(self, node: ast.If) -> None:
                # Mirror CPython's symbol table:
                # a name bound only in a statically-dead branch is still a local,
                # so this binding walk does NOT apply the static-if fold. The
                # fold is emission-only (`_emit_static_if_live_branch`).
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    for name in outer._collect_pattern_capture_names(case.pattern):
                        self._add(name)
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    self._add(node.name)
                self.generic_visit(node)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self._add_targets(target)

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name):
                    self._add(node.target.id)
                self.visit(node.value)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._add(node.name)
                for name in outer._collect_namedexpr_names(node):
                    self._add(name)

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._add(node.name)
                for name in outer._collect_namedexpr_names(node):
                    self._add(name)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self._add(node.name)
                for name in outer._collect_namedexpr_names(node):
                    self._add(name)

            def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
                if isinstance(node.name, ast.Name):
                    self._add(node.name.id)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                for name in outer._collect_namedexpr_names(node):
                    self._add(name)

            def visit_ListComp(
                self, node: ast.ListComp | ast.SetComp | ast.DictComp | ast.GeneratorExp
            ) -> None:
                for name in outer._collect_namedexpr_names(node):
                    self._add(name)

            def visit_SetComp(self, node: ast.SetComp) -> None:
                self.visit_ListComp(node)

            def visit_DictComp(self, node: ast.DictComp) -> None:
                self.visit_ListComp(node)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self.visit_ListComp(node)

            def visit_Import(self, node: ast.Import) -> None:
                # See `_collect_assigned_names`: imports bind names in the
                # current scope (CPython symbol table parity), so a
                # conditionally (re)imported name is an ordered binding of the
                # enclosing function/module for co_varnames and flush/evict.
                for alias in node.names:
                    self._add(alias.asname or alias.name.split(".", 1)[0])

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                for alias in node.names:
                    if alias.name == "*":
                        continue
                    self._add(alias.asname or alias.name)

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_varnames_for_body(
        self,
        *,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
        body: list[ast.stmt],
    ) -> list[str]:
        params = self._varnames_from_params(
            posonly_params=posonly_params,
            pos_or_kw_params=pos_or_kw_params,
            kwonly_params=kwonly_params,
            vararg=vararg,
            varkw=varkw,
        )
        assigned = self._collect_assigned_names_ordered(body)
        global_decls = self._collect_global_decls(body)
        nonlocal_decls = self._collect_nonlocal_decls(body)
        locals_only: list[str] = []
        for name in assigned:
            if (
                name in params
                or name in global_decls
                or name in nonlocal_decls
                or name in locals_only
            ):
                continue
            locals_only.append(name)
        return params + locals_only

    def _collect_code_names_for_body(
        self,
        nodes: Sequence[ast.AST],
        *,
        varnames: Sequence[str],
        free_vars: Sequence[str],
        module_scope: bool = False,
    ) -> list[str]:
        """Collect the ordered name table backing ``code.co_names``.

        The table is a runtime introspection fact, not an execution fallback:
        it mirrors the names referenced by bytecode-style name operations for
        the current code object while leaving nested code objects to describe
        their own bodies.
        """

        local_names = set(varnames)
        free_var_names = set(free_vars)
        stmt_nodes = [node for node in nodes if isinstance(node, ast.stmt)]
        global_decls = self._collect_global_decls(stmt_nodes)
        nonlocal_decls = self._collect_nonlocal_decls(stmt_nodes)
        outer = self
        names: list[str] = []
        seen: set[str] = set()

        def add(name: str | None) -> None:
            if not name or name in seen:
                return
            seen.add(name)
            names.append(name)

        def import_store_name(alias: ast.alias) -> str:
            if alias.asname:
                return alias.asname
            return alias.name.split(".", 1)[0]

        class CodeNamesCollector(PythonLexicalScopeVisitor):
            def visit_Name(self, node: ast.Name) -> None:
                if module_scope:
                    add(node.id)
                    return
                if node.id in nonlocal_decls or node.id in free_var_names:
                    return
                if node.id in global_decls:
                    add(node.id)
                    return
                if isinstance(node.ctx, ast.Load) and node.id not in local_names:
                    add(node.id)

            def visit_Attribute(self, node: ast.Attribute) -> None:
                self.visit(node.value)
                add(node.attr)

            def visit_Import(self, node: ast.Import) -> None:
                for alias in node.names:
                    add(alias.name)
                    if "." in alias.name:
                        if module_scope:
                            add(import_store_name(alias))
                        else:
                            add(alias.name.rsplit(".", 1)[1])
                    elif module_scope:
                        add(import_store_name(alias))

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                add("." * int(node.level or 0) + (node.module or ""))
                for alias in node.names:
                    add(alias.name)
                    if module_scope and alias.asname:
                        add(alias.asname)

            def visit_If(self, node: ast.If) -> None:
                static_branch = static_if_live_branch(
                    node,
                    **outer._static_truth_kwargs(),
                )
                if static_branch is not None:
                    if static_expression_result(
                        node.test, **outer._static_truth_kwargs()
                    ).evaluation_required:
                        self.visit(node.test)
                    for stmt in static_branch:
                        self.visit(stmt)
                    return None
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                super().visit_FunctionDef(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                super().visit_AsyncFunctionDef(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                super().visit_ClassDef(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

        collector = CodeNamesCollector(
            eager_annotations=self.eager_annotations and not self.future_annotations,
            variable_annotations=(
                module_scope and self.eager_annotations and not self.future_annotations
            ),
        )
        for node in nodes:
            collector.visit(node)
        return names

    def _collect_namedexpr_names(self, node: ast.AST) -> list[str]:
        # Source order, deduplicated.  Walrus (:=) targets are synced to the
        # enclosing scope by iterating this result and emitting INDEX / module-
        # attr-set ops per name (see _collect_inline_comp_walrus_names callers),
        # so a set leaked PYTHONHASHSEED order into the emitted IR (#34,
        # walrus-target class).  Set-semantics consumers wrap in set(...).
        names: list[str] = []
        seen: set[str] = set()

        def record(name: str) -> None:
            if name not in seen:
                seen.add(name)
                names.append(name)

        ScopedNamedExprCollector(
            record,
            eager_annotations=self.eager_annotations and not self.future_annotations,
        ).visit(node)
        return names

    def _collect_deleted_names(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        class DeleteCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                # `except E as e:` implicitly `del e` at handler exit (CPython
                # unconditionally deletes the target even when the handler body
                # raises). A subsequent read of `e` is therefore an unbound
                # name — NameError at module scope, UnboundLocalError in a
                # function — so the target must be tracked alongside explicit
                # `del` names to route post-block reads through the correct
                # unbound-name path rather than an attribute access.
                if node.name:
                    self.names.add(node.name)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = DeleteCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _lexical_dependencies(self) -> PythonDependencyAuthority:
        authority = self._lexical_dependency_cache
        if (
            authority is None
            or self._lexical_dependency_index is not self.python_binding_index
            or authority.eager_annotations != self.eager_annotations
            or authority.future_annotations != self.future_annotations
        ):
            index = self.python_binding_index

            def include_lexical_read(node: ast.Name) -> bool:
                fact = index.expression_fact(node) if index is not None else None
                return fact is None or fact.name_lookup not in {
                    "global",
                    "class_global",
                }

            authority = PythonDependencyAuthority(
                eager_annotations=self.eager_annotations,
                future_annotations=self.future_annotations,
                include_lexical_read=include_lexical_read,
            )
            self._lexical_dependency_cache = authority
            self._lexical_dependency_index = self.python_binding_index
        return authority

    def _cached_free_vars_raw(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
    ) -> frozenset[str]:
        return self._lexical_dependencies().summary(node).body.lexical

    def _free_vars_in_outer_scope(self, candidates: Iterable[str]) -> list[str]:
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[str]:
        return self._free_vars_in_outer_scope(self._cached_free_vars_raw(node))

    def _collect_free_vars_expr(self, node: ast.Lambda) -> list[str]:
        return self._free_vars_in_outer_scope(self._cached_free_vars_raw(node))

    def _collect_free_vars_raw(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> set[str]:
        return set(self._cached_free_vars_raw(node))

    def _collect_free_vars_expr_raw(self, node: ast.Lambda) -> set[str]:
        return set(self._cached_free_vars_raw(node))

    def _collect_free_vars_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
        authority = self._lexical_dependencies()
        candidates = set(authority.summary(node).body.lexical)
        return self._free_vars_in_outer_scope(candidates)

    def _collect_comprehension_cell_vars(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
        authority = self._lexical_dependencies()
        regions = authority.regions(node)
        return sorted(
            self._collect_scope_cell_vars(regions.body, set(regions.parameters))
        )

    def _collect_namedexpr_targets_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> set[str]:
        target_names: set[str] = set()
        exprs: list[ast.expr] = []
        for comp in node.generators:
            target_names.update(self._collect_target_names(comp.target))
            exprs.append(comp.iter)
            exprs.extend(comp.ifs)
        if isinstance(node, ast.DictComp):
            exprs.append(node.key)
            exprs.append(node.value)
        else:
            exprs.append(node.elt)
        names: set[str] = set()
        for expr in exprs:
            names |= set(self._collect_namedexpr_names(expr))
        names -= target_names
        return names

    def _collect_scope_cell_vars(
        self, body: Sequence[ast.AST], local_candidates: set[str]
    ) -> set[str]:
        if not local_candidates:
            return set()
        captured: set[str] = set()
        outer = self

        authority = self._lexical_dependencies()

        class Collector(PythonLexicalScopeVisitor):
            def __init__(self) -> None:
                super().__init__(
                    eager_annotations=outer.eager_annotations
                    and not outer.future_annotations
                )
                self.shadowed: set[str] = set()

            def _record(self, names: Iterable[str]) -> None:
                for name in names:
                    if name in local_candidates and name not in self.shadowed:
                        captured.add(name)

            def _visit_definition_header(self, node: LexicalDefinitionNode) -> None:
                regions = authority.regions(node)
                summary = authority.summary(node)
                self._record(
                    (summary.body.lexical | summary.annotations.lexical)
                    - regions.type_parameters
                )
                for expression in regions.enclosing:
                    self.visit(expression)

            def visit_Call(self, node: ast.Call) -> None:
                if (
                    isinstance(node.func, ast.Name)
                    and len(node.args) == 1
                    and not node.keywords
                    and isinstance(node.args[0], ast.GeneratorExp)
                    and (
                        (
                            node.func.id == "sum"
                            and outer._can_inline_sum_genexpr(node.args[0])
                        )
                        or (
                            node.func.id in {"any", "all"}
                            and outer._can_inline_any_all_genexpr(node.args[0])
                        )
                    )
                ):
                    self._visit_inline_comprehension(node.args[0])
                    return
                self.generic_visit(node)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self._record(authority.summary(node).body.lexical)
                for expression in authority.regions(node).enclosing:
                    self.visit(expression)

            def _visit_inline_comprehension(
                self, node: ast.ListComp | ast.SetComp | ast.DictComp | ast.GeneratorExp
            ) -> None:
                # PEP 709 has scoped bindings but no child code object. Only
                # true nested closures capture cells. The first iterable is
                # evaluated before targets hide the enclosing frame's names.
                regions = authority.regions(node)
                for expression in regions.enclosing:
                    self.visit(expression)
                previous = self.shadowed
                self.shadowed = previous | set(regions.parameters)
                try:
                    for expression in regions.body:
                        self.visit(expression)
                finally:
                    self.shadowed = previous

            def visit_ListComp(self, node: ast.ListComp) -> None:
                self._visit_inline_comprehension(node)

            def visit_SetComp(self, node: ast.SetComp) -> None:
                self._visit_inline_comprehension(node)

            def visit_DictComp(self, node: ast.DictComp) -> None:
                self._visit_inline_comprehension(node)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self._visit_definition_header(node)

        collector = Collector()
        for stmt in body:
            collector.visit(stmt)
        return captured

    def _collect_comp_walrus_cell_names(self, body: Sequence[ast.stmt]) -> list[str]:
        """Names whose comprehension walrus bindings require function cells.

        A walrus inside a comprehension leaks its binding to the enclosing
        function scope (PEP 572), but the inline-comprehension lowering stores
        that target through a boxed cell. The cell must exist at function entry
        even when the comprehension is nested in a zero-trip loop or untaken
        branch: the leaked binding remains an unbound local on that path, and
        later reads/cleanup still need one dominating storage identity. Boxing
        only names that also have a non-comprehension writer left walrus-only
        cells conditionally defined. Nested functions/classes remain separate
        scopes and are not traversed.
        """

        comp_walrus: set[str] = set()

        class _Scan(ast.NodeVisitor):
            def __init__(self) -> None:
                self._in_comp_depth = 0

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name):
                    if self._in_comp_depth > 0:
                        comp_walrus.add(node.target.id)
                self.visit(node.value)

            def _visit_comprehension(
                self,
                node: ast.ListComp | ast.SetComp | ast.GeneratorExp | ast.DictComp,
                parts: Sequence[ast.expr],
            ) -> None:
                # The iterable of the *first* generator is evaluated in the
                # enclosing scope; everything else (element, filters, nested
                # generators) is comprehension-internal for walrus-leak purposes.
                # Every caller passes a comprehension node, all four of which
                # carry ``.generators``.
                generators = node.generators
                if generators:
                    self.visit(generators[0].iter)
                self._in_comp_depth += 1
                try:
                    for part in parts:
                        self.visit(part)
                    for idx, comp in enumerate(generators):
                        if idx != 0:
                            self.visit(comp.iter)
                        for if_node in comp.ifs:
                            self.visit(if_node)
                finally:
                    self._in_comp_depth -= 1

            def visit_ListComp(self, node: ast.ListComp) -> None:
                self._visit_comprehension(node, [node.elt])

            def visit_SetComp(self, node: ast.SetComp) -> None:
                self._visit_comprehension(node, [node.elt])

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self._visit_comprehension(node, [node.elt])

            def visit_DictComp(self, node: ast.DictComp) -> None:
                self._visit_comprehension(node, [node.key, node.value])

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        scanner = _Scan()
        for stmt in body:
            scanner.visit(stmt)
        comp_walrus -= self.global_decls
        comp_walrus -= self.nonlocal_decls
        return sorted(comp_walrus)

    def _collect_class_mutations(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        def record_target(target: ast.AST, names: set[str]) -> None:
            if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name):
                class_name = target.value.id
                if class_name in outer.classes:
                    names.add(class_name)
            elif isinstance(target, ast.Starred):
                record_target(target.value, names)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt, names)

        class ClassMutationCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target, self.names)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target, self.names)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target, self.names)
                self.generic_visit(node.value)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target, self.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = ClassMutationCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_loop_guard_candidates(self, body: list[ast.stmt]) -> dict[str, str]:
        if self.is_async():
            return {}
        assigned = self._collect_assigned_names(body)
        mutated_classes = self._collect_class_mutations(body)
        attr_names: set[str] = set()

        class AttrCollector(ast.NodeVisitor):
            def visit_Attribute(self, node: ast.Attribute) -> None:
                if isinstance(node.value, ast.Name) and isinstance(node.ctx, ast.Load):
                    attr_names.add(node.value.id)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AttrCollector()
        for stmt in body:
            collector.visit(stmt)
        candidates: dict[str, str] = {}
        for name in sorted(attr_names):
            if name in assigned:
                continue
            expected_class = self.exact_locals.get(name)
            if expected_class is None:
                continue
            if expected_class in mutated_classes:
                continue
            candidates[name] = expected_class
        return candidates

    def _collect_loop_static_class_candidates(self, body: list[ast.stmt]) -> list[str]:
        if (
            self.is_async()
            or self.current_func_name == "molt_main"
            or not self.stable_module_classes
        ):
            return []
        assigned = self._collect_assigned_names(body)
        assigned |= {
            name for stmt in body for name in self._collect_namedexpr_names(stmt)
        }
        candidates: set[str] = set()
        outer = self

        class ClassCallCollector(ast.NodeVisitor):
            def visit_Call(self, node: ast.Call) -> None:
                if isinstance(node.func, ast.Name):
                    class_name = node.func.id
                    if (
                        class_name in outer.stable_module_classes
                        and class_name not in assigned
                        and class_name not in outer.scope_assigned
                        and class_name not in outer.global_decls
                        and outer._class_layout_stable(class_name)
                    ):
                        candidates.add(class_name)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = ClassCallCollector()
        for stmt in body:
            collector.visit(stmt)
        return sorted(candidates)

    def _collect_target_names(self, target: ast.AST) -> list[str]:
        # Source (left-to-right) order, deduplicated.  A set would be lossy: its
        # iteration order is PYTHONHASHSEED-dependent, and several callers feed
        # these names positionally into emitted IR (e.g. the co_varnames tuple
        # via _collect_assigned_names_ordered), so a set leaked hash order into
        # the compiled output (#34, unpack-target class).  Returning an ordered
        # list keeps that deterministic; set-semantics callers wrap in set(...).
        if isinstance(target, ast.Name):
            return [target.id]
        if isinstance(target, ast.Starred):
            return self._collect_target_names(target.value)
        if isinstance(target, (ast.Tuple, ast.List)):
            names: list[str] = []
            seen: set[str] = set()
            for elt in target.elts:
                for name in self._collect_target_names(elt):
                    if name not in seen:
                        seen.add(name)
                        names.append(name)
            return names
        return []

    def _collect_global_decls(self, nodes: list[ast.stmt]) -> set[str]:
        class GlobalCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Global(self, node: ast.Global) -> None:
                self.names.update(node.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = GlobalCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_stable_module_classes(self, node: ast.Module) -> set[str]:
        if self._module_globals_dict_escapes(node):
            return set()
        class_defs: dict[str, int] = {}
        rebound: set[str] = set()
        deleted: set[str] = set()
        global_decls: set[str] = set()

        def record_target(target: ast.AST, names: set[str]) -> None:
            if isinstance(target, ast.Name):
                names.add(target.id)
                return
            if isinstance(target, ast.Starred):
                record_target(target.value, names)
                return
            if isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt, names)

        for stmt in node.body:
            if isinstance(stmt, ast.ClassDef):
                class_defs[stmt.name] = class_defs.get(stmt.name, 0) + 1
                continue
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if stmt.name in class_defs:
                    rebound.add(stmt.name)
                global_decls.update(self._collect_global_decls(stmt.body))
                continue
            if isinstance(stmt, ast.Assign):
                for target in stmt.targets:
                    record_target(target, rebound)
                continue
            if isinstance(stmt, ast.AnnAssign):
                record_target(stmt.target, rebound)
                continue
            if isinstance(stmt, ast.AugAssign):
                record_target(stmt.target, rebound)
                continue
            if isinstance(stmt, ast.Delete):
                for target in stmt.targets:
                    record_target(target, deleted)
                continue
            if isinstance(stmt, (ast.Import, ast.ImportFrom)):
                for alias in stmt.names:
                    rebound.add(alias.asname or alias.name.split(".", 1)[0])

        return {
            name
            for name, count in class_defs.items()
            if count == 1
            and name not in rebound
            and name not in deleted
            and name not in global_decls
        }

    def _collect_nonlocal_decls(self, nodes: list[ast.stmt]) -> set[str]:
        class NonlocalCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
                self.names.update(node.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = NonlocalCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_inline_comp_walrus_names(
        self, exprs: Sequence[ast.AST], ifs: Sequence[ast.AST]
    ) -> list[str]:
        # Source order, deduplicated (deterministic): the result drives boxing
        # and the post-loop walrus-target sync emission, so it must not depend
        # on hash order (#34).
        walrus_names: list[str] = []
        seen: set[str] = set()
        for node in (*exprs, *ifs):
            for name in self._collect_namedexpr_names(node):
                if name not in seen:
                    seen.add(name)
                    walrus_names.append(name)
        return walrus_names

    def _collect_inline_comp_lambda_free_vars(
        self, exprs: Sequence[ast.AST], ifs: Sequence[ast.AST]
    ) -> set[str]:
        lambda_free_vars: set[str] = set()
        for root in [*exprs, *ifs]:
            for child in ast.walk(root):
                if isinstance(
                    child, (ast.Lambda, ast.FunctionDef, ast.AsyncFunctionDef)
                ):
                    for inner in ast.walk(child):
                        if isinstance(inner, ast.Name) and isinstance(
                            inner.ctx, ast.Load
                        ):
                            lambda_free_vars.add(inner.id)
        return lambda_free_vars

    def _collect_arg_value_names(self, value: Any, out: set[str]) -> None:
        if isinstance(value, MoltValue):
            out.add(value.name)
            return
        if isinstance(value, list):
            for item in value:
                self._collect_arg_value_names(item, out)
            return
        if isinstance(value, tuple):
            for item in value:
                self._collect_arg_value_names(item, out)
            return
        if isinstance(value, dict):
            for key, item in value.items():
                self._collect_arg_value_names(key, out)
                self._collect_arg_value_names(item, out)

    def _collect_defined_value_names(self, ops: list[MoltOp]) -> set[str]:
        defined: set[str] = set()
        for op in ops:
            out_name = op.result.name
            if out_name != "none":
                defined.add(out_name)
        return defined

    def _collect_branch_defined_names(self, ops: list[MoltOp]) -> set[str]:
        out: set[str] = set()
        for op in ops:
            if op.result.name != "none":
                out.add(op.result.name)
        return out

    def _collect_movable_common_guards(
        self, then_ops: list[MoltOp], else_ops: list[MoltOp]
    ) -> list[MoltOp]:
        then_defined = self._collect_branch_defined_names(then_ops)
        else_defined = self._collect_branch_defined_names(else_ops)
        branch_defined = then_defined.union(else_defined)

        def candidates(ops: list[MoltOp]) -> dict[tuple[Any, ...], MoltOp]:
            found: dict[tuple[Any, ...], MoltOp] = {}
            for op in ops:
                sig = self._guard_signature(op)
                if sig is None:
                    continue
                arg_names: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, arg_names)
                if arg_names.intersection(branch_defined):
                    continue
                found.setdefault(sig, op)
            return found

        then_guards = candidates(then_ops)
        else_guards = candidates(else_ops)
        common_sigs = sorted(set(then_guards.keys()).intersection(else_guards.keys()))
        hoisted: list[MoltOp] = []
        for sig in common_sigs:
            source = then_guards[sig]
            hoisted.append(
                MoltOp(
                    kind=source.kind,
                    args=list(source.args),
                    result=MoltValue("none"),
                    metadata=source.metadata,
                )
            )
        return hoisted
