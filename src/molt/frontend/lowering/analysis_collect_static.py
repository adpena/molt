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

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AnalysisCollectStaticMixin(_MixinBase):
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
                static_branch = outer._static_if_live_branch(node)
                if static_branch is not None:
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

        def record_pattern(pattern: ast.pattern) -> None:
            if isinstance(pattern, ast.MatchAs):
                if pattern.name and pattern.name != "_":
                    record(pattern.name)
                if pattern.pattern is not None:
                    record_pattern(pattern.pattern)
            elif isinstance(pattern, ast.MatchStar):
                if pattern.name and pattern.name != "_":
                    record(pattern.name)
            elif isinstance(pattern, ast.MatchMapping):
                for sub in pattern.patterns:
                    record_pattern(sub)
                if pattern.rest and pattern.rest != "_":
                    record(pattern.rest)
            elif isinstance(pattern, ast.MatchSequence):
                for sub in pattern.patterns:
                    record_pattern(sub)
            elif isinstance(pattern, ast.MatchClass):
                for sub in pattern.patterns:
                    record_pattern(sub)
                for sub in pattern.kwd_patterns:
                    record_pattern(sub)
            elif isinstance(pattern, ast.MatchOr):
                for sub in pattern.patterns:
                    record_pattern(sub)

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                return None

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                return None

            def visit_ClassDef(self, node: ast.ClassDef) -> Any:
                record(node.name)
                return None

            def visit_Lambda(self, node: ast.Lambda) -> Any:
                return None

            def visit_ListComp(self, node: ast.ListComp) -> Any:
                return None

            def visit_SetComp(self, node: ast.SetComp) -> Any:
                return None

            def visit_DictComp(self, node: ast.DictComp) -> Any:
                return None

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
                return None

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                record_target(node.target)
                self.visit(node.value)

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
                static_branch = outer._static_if_live_branch(node)
                if static_branch is not None:
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

        collector = Collector()
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

    def _collect_annotation_free_vars(self, node: ast.AST) -> list[str]:
        if self.current_func_name == "molt_main":
            return []
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> None:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node)
        used -= self.global_decls
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in used if name in outer_scope)

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
        # Source order, deduplicated.  A set leaked PYTHONHASHSEED ordering into
        # emitted IR because _collect_assigned_names_ordered feeds these capture
        # names positionally into the function's co_varnames tuple (#34,
        # match-capture class).  Callers that need set semantics (e.g. MatchOr
        # binding-equality) wrap in set(...).
        names: list[str] = []
        seen: set[str] = set()

        def add(name: str) -> None:
            if name not in seen:
                seen.add(name)
                names.append(name)

        def visit(current: ast.pattern) -> None:
            if isinstance(current, ast.MatchAs):
                if current.name and current.name != "_":
                    add(current.name)
                if current.pattern is not None:
                    visit(current.pattern)
                return
            if isinstance(current, ast.MatchStar):
                if current.name and current.name != "_":
                    add(current.name)
                return
            if isinstance(current, ast.MatchMapping):
                for sub in current.patterns:
                    visit(sub)
                if current.rest and current.rest != "_":
                    add(current.rest)
                return
            if isinstance(current, ast.MatchSequence):
                for sub in current.patterns:
                    visit(sub)
                return
            if isinstance(current, ast.MatchClass):
                for sub in current.patterns:
                    visit(sub)
                for sub in current.kwd_patterns:
                    visit(sub)
                return
            if isinstance(current, ast.MatchOr):
                for sub in current.patterns:
                    visit(sub)
                return

        visit(pattern)
        return names

    def _collect_assigned_names(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self.names.update(outer._collect_target_names(node.target))
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self.names.update(
                            outer._collect_target_names(item.optional_vars)
                        )
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self.names.update(
                            outer._collect_target_names(item.optional_vars)
                        )
                self.generic_visit(node)

            def visit_If(self, node: ast.If) -> None:
                # Binding analysis mirrors CPython's symbol table, which records
                # every assignment target regardless of static reachability: a
                # name bound only in a statically-dead branch (`if 0: x = 1`) is
                # still a local of the enclosing scope, so reading it raises
                # UnboundLocalError, not NameError. The static-if fold is a
                # codegen/emission concern (drop dead-branch *code* and its
                # const_str/intrinsic refs), handled in the emission `visit_If`
                # via `_emit_static_if_live_branch`; pruning scope bindings here
                # would diverge from CPython and is intentionally NOT done.
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    self.names.update(
                        outer._collect_pattern_capture_names(case.pattern)
                    )
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    self.names.add(node.name)
                self.generic_visit(node)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self.names.add(node.name)
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self.names.add(node.name)
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self.names.add(node.name)
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_assigned_names_ordered(self, nodes: list[ast.stmt]) -> list[str]:
        outer = self

        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: list[str] = []
                self.seen: set[str] = set()

            def _add(self, name: str) -> None:
                if name not in self.seen:
                    self.seen.add(name)
                    self.names.append(name)

            def _add_targets(self, target: ast.AST) -> None:
                for name in outer._collect_target_names(target):
                    self._add(name)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self._add_targets(target)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self._add_targets(node.target)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self._add_targets(node.target)
                self.generic_visit(node.value)

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
                # Mirror CPython's symbol table (see `_collect_assigned_names`):
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
                self.generic_visit(node.value)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._add(node.name)
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._add(node.name)
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self._add(node.name)
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

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

        class CodeNamesCollector(ast.NodeVisitor):
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

            def _visit_function_signature(
                self, node: ast.FunctionDef | ast.AsyncFunctionDef
            ) -> None:
                for deco in node.decorator_list:
                    self.visit(deco)
                for default in node.args.defaults:
                    self.visit(default)
                for default in node.args.kw_defaults:
                    if default is not None:
                        self.visit(default)
                for arg in (
                    list(node.args.posonlyargs)
                    + list(node.args.args)
                    + list(node.args.kwonlyargs)
                ):
                    if arg.annotation is not None:
                        self.visit(arg.annotation)
                if (
                    node.args.vararg is not None
                    and node.args.vararg.annotation is not None
                ):
                    self.visit(node.args.vararg.annotation)
                if (
                    node.args.kwarg is not None
                    and node.args.kwarg.annotation is not None
                ):
                    self.visit(node.args.kwarg.annotation)
                if node.returns is not None:
                    self.visit(node.returns)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._visit_function_signature(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._visit_function_signature(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                for default in node.args.defaults:
                    self.visit(default)
                for default in node.args.kw_defaults:
                    if default is not None:
                        self.visit(default)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                for deco in node.decorator_list:
                    self.visit(deco)
                for base in node.bases:
                    self.visit(base)
                for keyword in node.keywords:
                    self.visit(keyword.value)
                if module_scope or node.name in global_decls:
                    add(node.name)

        collector = CodeNamesCollector()
        for node in nodes:
            collector.visit(node)
        return names

    @staticmethod
    def _is_type_checking_test(expr: ast.expr) -> bool:
        if isinstance(expr, ast.Name):
            return expr.id == "TYPE_CHECKING"
        if isinstance(expr, ast.Attribute):
            if expr.attr != "TYPE_CHECKING":
                return False
            if isinstance(expr.value, ast.Name):
                return expr.value.id in {"typing", "typing_extensions"}
        return False

    @staticmethod
    def _static_test_truthiness(expr: ast.expr) -> bool | None:
        """Return the compile-time truth value of an `if`/`while` test, or None.

        CPython's compiler eliminates the dead branch of an `if` whose test is a
        compile-time constant (`if False:`, `if 0:`, `if "":`, `if True:`,
        `if None:`), so the dead branch never reaches bytecode — names assigned
        only there stay unbound and references inside it are never emitted. Molt
        must match this exactly: a `const_str` left inside a never-executed
        `if False:` body (e.g. the `__annotations__` keys of a
        `if False:  # TYPE_CHECKING` block) would otherwise leak into the
        per-app intrinsic manifest and pin runtime intrinsics that the program
        never resolves.

        `TYPE_CHECKING` is always statically False here: Molt compiles code, it
        never runs a type checker, so a `if TYPE_CHECKING:` guard's body is dead
        exactly like `if False:`. Returning False for it unifies the existing
        TYPE_CHECKING-skip with general constant folding (one code path, not two).

        Returns None when the test is not a compile-time constant — the caller
        must then emit both branches under a runtime guard.
        """
        if AnalysisCollectStaticMixin._is_type_checking_test(expr):
            return False
        if isinstance(expr, ast.Constant):
            # Mirror CPython's constant folding: any literal test value collapses
            # to its truthiness (None/bool/int/float/str/bytes/tuple-of-consts).
            return bool(expr.value)
        return None

    @staticmethod
    def _static_if_live_branch(node: ast.If) -> list[ast.stmt] | None:
        """Statically-live branch of `node` when its test is constant, else None.

        Constant-true selects `node.body`; constant-false (including
        `TYPE_CHECKING`) selects `node.orelse`. None means the test is
        runtime-conditional and both branches may execute.
        """
        truth = AnalysisCollectStaticMixin._static_test_truthiness(node.test)
        if truth is None:
            return None
        return node.body if truth else node.orelse

    def _collect_namedexpr_names(self, node: ast.AST) -> list[str]:
        # Source order, deduplicated.  Walrus (:=) targets are synced to the
        # enclosing scope by iterating this result and emitting INDEX / module-
        # attr-set ops per name (see _collect_inline_comp_walrus_names callers),
        # so a set leaked PYTHONHASHSEED order into the emitted IR (#34,
        # walrus-target class).  Set-semantics consumers wrap in set(...).
        names: list[str] = []
        seen: set[str] = set()

        class NamedExprCollector(ast.NodeVisitor):
            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name) and node.target.id not in seen:
                    seen.add(node.target.id)
                    names.append(node.target.id)
                self.generic_visit(node.value)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        NamedExprCollector().visit(node)
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

    def _collect_free_vars(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names(node.body)
        comp_targets = self._collect_comprehension_target_names(node.body)
        global_decls = self._collect_global_decls(node.body)
        nonlocal_decls = self._collect_nonlocal_decls(node.body)
        local_names = params | comp_targets | (assigned - nonlocal_decls)
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        used.update(nonlocal_decls)
        used.update(self._collect_nested_free_vars(node.body))
        # Implicit ``__class__`` closure variable: a method/nested function
        # that references zero-arg ``super()`` or ``__class__`` closes over the
        # enclosing class's ``__class__`` cell exactly as CPython does.  The
        # cell lives in ``self.boxed_locals['__class__']`` (pre-created by
        # visit_ClassDef), so adding ``__class__`` here threads it through the
        # closure and lets ``super()``/``__class__`` read the finished class
        # object from the cell rather than re-deriving it by module name.
        if self._active_classcell_cell is not None and self._function_needs_classcell(
            node
        ):
            used.add("__class__")
        candidates = {
            name
            for name in used
            if name not in local_names and name not in global_decls
        }
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars_expr(self, node: ast.Lambda) -> list[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
        comp_targets = self._collect_comprehension_target_names([node.body])
        local_names = params | comp_targets | assigned
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node.body)
        used.update(self._collect_nested_free_vars([node.body]))
        candidates = {name for name in used if name not in local_names}
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars_raw(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> set[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names(node.body)
        comp_targets = self._collect_comprehension_target_names(node.body)
        global_decls = self._collect_global_decls(node.body)
        nonlocal_decls = self._collect_nonlocal_decls(node.body)
        local_names = params | comp_targets | (assigned - nonlocal_decls)
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        used.update(nonlocal_decls)
        used.update(self._collect_nested_free_vars_raw(node.body))
        return {
            name
            for name in used
            if name not in local_names and name not in global_decls
        }

    def _collect_free_vars_expr_raw(self, node: ast.Lambda) -> set[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
        comp_targets = self._collect_comprehension_target_names([node.body])
        local_names = params | comp_targets | assigned
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node.body)
        used.update(self._collect_nested_free_vars_raw([node.body]))
        return {name for name in used if name not in local_names}

    def _collect_free_vars_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
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
        namedexpr_targets: set[str] = set()
        for expr in exprs:
            namedexpr_targets |= set(self._collect_namedexpr_names(expr))
        assigned = self._collect_assigned_names(
            [ast.Expr(value=expr) for expr in exprs]
        )
        local_names = target_names | assigned
        used: set[str] = set()
        # Capture the method's first param name so we can detect implicit
        # super() references inside comprehensions.
        _method_first_param = self.current_method_first_param
        _current_class = self.current_class

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)
                    if (
                        node.id == "super"
                        and _method_first_param is not None
                        and _current_class is not None
                    ):
                        used.add(_method_first_param)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for expr in exprs:
            collector.visit(expr)
        used |= namedexpr_targets
        used.update(self._collect_nested_free_vars(exprs))
        candidates = {name for name in used if name not in local_names}
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_nested_free_vars(self, nodes: Sequence[ast.AST]) -> set[str]:
        nested: set[str] = set()
        outer = self

        class NestedCollector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested.update(outer._collect_free_vars(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested.update(outer._collect_free_vars(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested.update(outer._collect_free_vars_expr(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = NestedCollector()
        for node in nodes:
            collector.visit(node)
        return nested

    def _collect_nested_free_vars_raw(self, nodes: Sequence[ast.AST]) -> set[str]:
        nested: set[str] = set()
        outer = self

        class NestedCollector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested.update(outer._collect_free_vars_raw(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested.update(outer._collect_free_vars_raw(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested.update(outer._collect_free_vars_expr_raw(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = NestedCollector()
        for node in nodes:
            collector.visit(node)
        return nested

    def _collect_comprehension_cell_vars(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
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
        nested_free: set[str] = set()
        outer = self

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested_free.update(outer._collect_free_vars_raw(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested_free.update(outer._collect_free_vars_raw(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested_free.update(outer._collect_free_vars_expr_raw(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = Collector()
        for expr in exprs:
            collector.visit(expr)
        return sorted(name for name in nested_free if name in target_names)

    def _collect_comprehension_target_names(self, nodes: Sequence[ast.AST]) -> set[str]:
        names: set[str] = set()
        outer = self

        class Collector(ast.NodeVisitor):
            def visit_ListComp(self, node: ast.ListComp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_SetComp(self, node: ast.SetComp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_DictComp(self, node: ast.DictComp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for node in nodes:
            collector.visit(node)
        return names

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
        self, body: Sequence[ast.stmt], local_candidates: set[str]
    ) -> set[str]:
        if not local_candidates:
            return set()
        captured: set[str] = set()
        outer = self

        class Collector(ast.NodeVisitor):
            def _record(self, names: Iterable[str]) -> None:
                for name in names:
                    if name in local_candidates:
                        captured.add(name)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._record(outer._collect_free_vars_raw(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._record(outer._collect_free_vars_raw(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                self._record(outer._collect_free_vars_expr_raw(node))
                return

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
                    genexpr = node.args[0]
                    for comp in genexpr.generators:
                        self.visit(comp.iter)
                        for if_node in comp.ifs:
                            self.visit(if_node)
                    self.visit(genexpr.elt)
                    return
                self.generic_visit(node)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_ListComp(self, node: ast.ListComp) -> None:
                if not (
                    not outer._comprehension_requires_async(node.generators, [node.elt])
                    and outer._can_inline_list_comp(node)
                ):
                    self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_SetComp(self, node: ast.SetComp) -> None:
                if not (
                    not outer._comprehension_requires_async(node.generators, [node.elt])
                    and outer._can_inline_set_comp(node)
                ):
                    self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_DictComp(self, node: ast.DictComp) -> None:
                if not (
                    not outer._comprehension_requires_async(
                        node.generators, [node.key, node.value]
                    )
                    and outer._can_inline_dict_comp(node)
                ):
                    self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = Collector()
        for stmt in body:
            collector.visit(stmt)
        return captured

    def _collect_comp_walrus_shared_names(self, body: Sequence[ast.stmt]) -> list[str]:
        """Names that are a comprehension walrus (``:=``) target AND are also
        bound by a non-comprehension assignment in the same function scope.

        A walrus inside a comprehension leaks its binding to the enclosing
        function scope (PEP 572), but the inline-comprehension lowering stores
        that target through a boxed cell while a *separate* binding of the same
        name (a plain assignment, a ``while``/``if`` test walrus, a ``for``
        target, ...) is lowered as a plain SSA local.  When such a name lives
        across a loop back-edge the two representations diverge — the comp cell
        is never updated by the SSA writer and vice-versa — producing a stale
        post-loop value (e.g. ``while (n := next(it)) is not None: xs = [n := n
        + 1 for _ in r]`` leaving ``n`` at the last comp value instead of the
        loop-terminating ``None``).  Returning such names lets the caller box
        them at function entry so every binding site shares one cell.

        Names bound *only* by a comprehension walrus are excluded: their cell is
        the single source of truth (the post-comp sync mirrors it into the SSA
        local) and needs no unification.  Nested functions/classes are separate
        scopes and are not traversed.
        """

        outer = self

        comp_walrus: set[str] = set()
        non_comp_assigned: set[str] = set()

        class _Scan(ast.NodeVisitor):
            def __init__(self) -> None:
                self._in_comp_depth = 0

            def _record_assign_targets(self, target: ast.expr) -> None:
                for name in outer._collect_target_names(target):
                    non_comp_assigned.add(name)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self._record_assign_targets(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self._record_assign_targets(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self._record_assign_targets(node.target)
                self.visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self._record_assign_targets(node.target)
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self._record_assign_targets(node.target)
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._record_assign_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._record_assign_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name):
                    if self._in_comp_depth > 0:
                        comp_walrus.add(node.target.id)
                    else:
                        non_comp_assigned.add(node.target.id)
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
        shared = comp_walrus & non_comp_assigned
        shared -= self.global_decls
        shared -= self.nonlocal_decls
        return sorted(shared)

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
