"""Shared local-Python import resolution for compiler cache authorities."""

from __future__ import annotations

import ast
import tokenize
from dataclasses import dataclass, field
from pathlib import Path
from threading import RLock
from typing import Iterable, Literal

from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    analyze_module_import_flow,
    bind_static_import_call_arguments,
    dunder_globals_state_from_expression,
    metadata_value_from_expression,
    project_static_import_request,
)

@dataclass(frozen=True)
class PythonImportPolicy:
    """Resolution policy for one import-closure consumer.

    Frontend semantic fingerprints intentionally follow module-level ``molt``
    edges without executing package aggregates. Executable tools need the full
    lexical graph and CPython's parent-package execution semantics. Keeping the
    distinction as data prevents the two consumers from growing separate path
    and relative-import resolvers again.
    """

    module_level_only: bool
    include_parent_packages: bool
    fail_on_nonliteral_dynamic_import: bool
    allowed_prefix: str | None = None


@dataclass(frozen=True)
class _ResolvedLocalModule:
    source: Path | None
    package_locations: tuple[Path, ...]
    parent_initializers: tuple[Path, ...]


@dataclass(frozen=True)
class LocalPythonModuleResolver:
    search_roots: tuple[Path, ...]
    _resolution_cache: dict[str, _ResolvedLocalModule | None] = field(
        default_factory=dict,
        compare=False,
        repr=False,
    )
    _cache_lock: RLock = field(default_factory=RLock, compare=False, repr=False)

    def __post_init__(self) -> None:
        resolved = tuple(root.resolve() for root in self.search_roots)
        if not resolved:
            raise ValueError("local Python module resolver requires a search root")
        object.__setattr__(self, "search_roots", resolved)

    def read_ast(self, path: Path) -> ast.Module:
        try:
            with tokenize.open(path) as stream:
                source = stream.read()
            return ast.parse(source, filename=str(path))
        except (OSError, SyntaxError, UnicodeError, ValueError) as exc:
            raise ValueError(f"cannot parse local Python source {path}: {exc}") from exc

    def module_identity(self, path: Path) -> tuple[str, str]:
        resolved = path.resolve()
        for root in self.search_roots:
            try:
                relative = resolved.relative_to(root)
            except ValueError:
                continue
            parts = list(relative.parts)
            is_package = parts[-1] == "__init__.py"
            if is_package:
                parts.pop()
            elif parts[-1].endswith(".py"):
                parts[-1] = parts[-1][: -len(".py")]
            else:
                raise ValueError(f"local Python source is not a .py file: {resolved}")
            module = ".".join(parts)
            package = module if is_package else module.rpartition(".")[0]
            return module, package
        raise ValueError(f"Python source is outside local search roots: {resolved}")

    def source_for_module(self, module: str) -> Path | None:
        resolution = self._resolve_module(module)
        return resolution.source if resolution is not None else None

    def _owned_path(self, path: Path) -> Path | None:
        """Resolve a candidate only when it remains inside a search root."""

        try:
            resolved = path.resolve()
        except OSError:
            return None
        if any(resolved.is_relative_to(root) for root in self.search_roots):
            return resolved
        return None

    def _resolve_module(self, module: str) -> _ResolvedLocalModule | None:
        if not module:
            return None
        parts = module.split(".")
        if any(not part or "/" in part or "\\" in part for part in parts):
            raise ValueError(f"invalid local Python module name: {module!r}")
        with self._cache_lock:
            if module in self._resolution_cache:
                return self._resolution_cache[module]

            # Mirror PathFinder one segment at a time. Namespace portions keep
            # searching; the first regular package or module stops the current
            # segment's search. A regular package then owns the next segment's
            # search path, while a regular module cannot have children.
            locations = self.search_roots
            parents: list[Path] = []
            for index, part in enumerate(parts):
                namespace_locations: list[Path] = []
                regular_source: Path | None = None
                regular_package_location: Path | None = None
                regular_is_package = False

                for location in locations:
                    package_location = self._owned_path(location / part)
                    if package_location is not None and package_location.is_dir():
                        initializer = self._owned_path(package_location / "__init__.py")
                        if initializer is not None and initializer.is_file():
                            regular_source = initializer
                            regular_package_location = package_location
                            regular_is_package = True
                            break
                        namespace_locations.append(package_location)

                    source = self._owned_path(location / f"{part}.py")
                    if source is not None and source.is_file():
                        regular_source = source
                        regular_package_location = None
                        regular_is_package = False
                        break

                final_segment = index == len(parts) - 1
                if regular_source is not None:
                    if final_segment:
                        result = _ResolvedLocalModule(
                            source=regular_source,
                            package_locations=(
                                (regular_package_location,)
                                if regular_package_location is not None
                                else ()
                            ),
                            parent_initializers=tuple(parents),
                        )
                        self._resolution_cache[module] = result
                        return result
                    if not regular_is_package or regular_package_location is None:
                        self._resolution_cache[module] = None
                        return None
                    parents.append(regular_source)
                    locations = (regular_package_location,)
                    continue

                if not namespace_locations:
                    self._resolution_cache[module] = None
                    return None
                locations = tuple(namespace_locations)
                if final_segment:
                    result = _ResolvedLocalModule(
                        source=None,
                        package_locations=locations,
                        parent_initializers=tuple(parents),
                    )
                    self._resolution_cache[module] = result
                    return result

            raise AssertionError("non-empty module resolution exhausted no segment")

    def resolve_with_owner_fallback(self, module: str) -> tuple[str, Path] | None:
        source = self.source_for_module(module)
        if source is not None:
            return module, source
        owner, separator, _name = module.rpartition(".")
        if not separator:
            return None
        source = self.source_for_module(owner)
        return (owner, source) if source is not None else None

    def parent_package_sources(self, module: str) -> tuple[Path, ...]:
        """Existing parent ``__init__`` files in CPython execution order.

        A missing initializer is a PEP 420 namespace package, not an error.
        The requested module/package itself is excluded; its source is returned
        separately by ``source_for_module``.
        """

        resolution = self._resolve_module(module)
        return resolution.parent_initializers if resolution is not None else ()


def _project_import_request(
    request: StaticImportRequest,
    context: ModuleImportContext,
    path: Path,
) -> tuple[str, ...]:
    projection = project_static_import_request(request, context)
    if projection.error == "no_parent":
        if request.kind == "import_module":
            raise ValueError(f"relative import_module requires a package in {path}")
        raise ValueError(f"relative import has no known parent package in {path}")
    if projection.error == "beyond_top":
        raise ValueError(f"relative import escapes local package in {path}")
    if projection.error == "empty_name":
        raise ValueError(f"empty Python module name in {path}")
    if projection.error == "negative_level":
        raise ValueError(f"negative __import__ level in {path}")
    if projection.error == "missing_globals":
        raise ValueError(f"relative __import__ requires explicit globals in {path}")
    if projection.error in {
        "invalid_package",
        "unknown_package",
        "invalid_spec",
        "unknown_spec",
    }:
        raise ValueError(f"non-literal or invalid import package in {path}")
    if projection.error == "missing_name":
        raise ValueError(f"relative import globals are missing __name__ in {path}")
    if projection.error in {"invalid_name", "unknown_name"}:
        raise ValueError(f"relative import has invalid or dynamic __name__ in {path}")
    return projection.modules


_AliasProvenance = str | None


@dataclass
class _LexicalImportScope:
    parent: _LexicalImportScope | None
    events: dict[
        str, list[tuple[tuple[int, int], _AliasProvenance, bool]]
    ] = field(
        default_factory=dict
    )

    def bind(
        self,
        name: str,
        provenance: _AliasProvenance,
        position: tuple[int, int],
        *,
        conditional: bool = False,
    ) -> None:
        self.events.setdefault(name, []).append((position, provenance, conditional))

    def resolve_all(
        self, name: str, position: tuple[int, int]
    ) -> frozenset[_AliasProvenance]:
        candidates = sorted(
            (
            event for event in self.events.get(name, ()) if event[0] <= position
            ),
            key=lambda event: event[0],
        )
        if candidates:
            possible: set[_AliasProvenance] = set()
            for _event_position, provenance, conditional in candidates:
                if not conditional:
                    possible = {provenance}
                else:
                    possible.add(provenance)
            return frozenset(possible)
        return (
            self.parent.resolve_all(name, position)
            if self.parent is not None
            else frozenset({None})
        )

    def resolve(self, name: str, position: tuple[int, int]) -> _AliasProvenance:
        possible = self.resolve_all(name, position)
        return next(iter(possible)) if len(possible) == 1 else None


def _binding_names(target: ast.AST | None) -> set[str]:
    if target is None:
        return set()
    if isinstance(target, ast.Name):
        return {target.id}
    if isinstance(target, (ast.Tuple, ast.List)):
        return {name for item in target.elts for name in _binding_names(item)}
    if isinstance(target, ast.Starred):
        return _binding_names(target.value)
    if isinstance(target, (ast.MatchAs, ast.MatchStar)):
        names = {target.name} if target.name else set()
        if isinstance(target, ast.MatchAs):
            names.update(_binding_names(target.pattern))
        return names
    if isinstance(target, ast.MatchMapping):
        names = {target.rest} if target.rest else set()
        for pattern in target.patterns:
            names.update(_binding_names(pattern))
        return names
    if isinstance(target, ast.MatchSequence):
        return {name for pattern in target.patterns for name in _binding_names(pattern)}
    if isinstance(target, ast.MatchClass):
        return {
            name
            for pattern in (*target.patterns, *target.kwd_patterns)
            for name in _binding_names(pattern)
        }
    if isinstance(target, ast.MatchOr):
        return {name for pattern in target.patterns for name in _binding_names(pattern)}
    return set()


class _FunctionLocalBindings(ast.NodeVisitor):
    def __init__(self) -> None:
        self.names: set[str] = set()
        self.globals: set[str] = set()
        self.nonlocals: set[str] = set()

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, (ast.Store, ast.Del)):
            self.names.add(node.id)

    def visit_Import(self, node: ast.Import) -> None:
        self.names.update(
            alias.asname or alias.name.split(".", 1)[0] for alias in node.names
        )

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self.names.update(
            alias.asname or alias.name for alias in node.names if alias.name != "*"
        )

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.names.add(node.name)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.names.add(node.name)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.names.add(node.name)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        return

    def visit_Global(self, node: ast.Global) -> None:
        self.globals.update(node.names)

    def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
        self.nonlocals.update(node.names)

    def visit_comprehension(self, node: ast.comprehension) -> None:
        self.visit(node.iter)
        for condition in node.ifs:
            self.visit(condition)


def _function_local_names(
    node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
) -> set[str]:
    collector = _FunctionLocalBindings()
    body = node.body if isinstance(node.body, list) else [node.body]
    for statement in body:
        collector.visit(statement)
    arguments = node.args
    collector.names.update(
        arg.arg
        for arg in (
            *arguments.posonlyargs,
            *arguments.args,
            *arguments.kwonlyargs,
        )
    )
    if arguments.vararg is not None:
        collector.names.add(arguments.vararg.arg)
    if arguments.kwarg is not None:
        collector.names.add(arguments.kwarg.arg)
    return collector.names - collector.globals - collector.nonlocals


def _node_end(node: ast.AST) -> tuple[int, int]:
    return (
        int(getattr(node, "end_lineno", getattr(node, "lineno", 0))),
        int(getattr(node, "end_col_offset", getattr(node, "col_offset", 0))),
    )


class _DynamicImportLexicalIndex(ast.NodeVisitor):
    def __init__(self, tree: ast.Module) -> None:
        self.scope = _LexicalImportScope(None)
        self.conditional_depth = 0
        self.scope.bind("__import__", "dunder_import", (-1, -1))
        self.calls: list[tuple[ast.Call, _LexicalImportScope]] = []
        self.visit(tree)

    def _bind_target(self, target: ast.AST | None, node: ast.AST) -> None:
        for name in _binding_names(target):
            self.scope.bind(
                name,
                None,
                _node_end(node),
                conditional=self.conditional_depth > 0,
            )

    def _visit_conditional(self, statements: Iterable[ast.AST]) -> None:
        self.conditional_depth += 1
        try:
            for statement in statements:
                self.visit(statement)
        finally:
            self.conditional_depth -= 1

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            name = alias.asname or alias.name.split(".", 1)[0]
            provenance = (
                "importlib"
                if alias.name == "importlib"
                or (alias.name.startswith("importlib.") and alias.asname is None)
                else None
            )
            self.scope.bind(
                name,
                provenance,
                _node_end(node),
                conditional=self.conditional_depth > 0,
            )

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        for alias in node.names:
            if alias.name == "*":
                continue
            name = alias.asname or alias.name
            provenance: _AliasProvenance = None
            if (
                node.level == 0
                and node.module == "importlib"
                and alias.name == "import_module"
            ):
                provenance = "import_module"
            elif (
                node.level == 0
                and node.module == "builtins"
                and alias.name == "__import__"
            ):
                provenance = "dunder_import"
            self.scope.bind(
                name,
                provenance,
                _node_end(node),
                conditional=self.conditional_depth > 0,
            )

    def visit_If(self, node: ast.If) -> None:
        self.visit(node.test)
        self._visit_conditional((*node.body, *node.orelse))

    def visit_While(self, node: ast.While) -> None:
        self.visit(node.test)
        self._visit_conditional((*node.body, *node.orelse))

    def visit_Try(self, node: ast.Try) -> None:
        self._visit_conditional(node.body)
        for handler in node.handlers:
            self._visit_conditional((handler,))
        self._visit_conditional(node.orelse)
        for statement in node.finalbody:
            self.visit(statement)

    def visit_Assign(self, node: ast.Assign) -> None:
        self.visit(node.value)
        for target in node.targets:
            self._bind_target(target, node)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        self.visit(node.annotation)
        if node.value is not None:
            self.visit(node.value)
        self._bind_target(node.target, node)

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        self.visit(node.target)
        self.visit(node.value)
        self._bind_target(node.target, node)

    def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
        self.visit(node.value)
        self._bind_target(node.target, node)

    def visit_Delete(self, node: ast.Delete) -> None:
        for target in node.targets:
            self._bind_target(target, node)

    def visit_For(self, node: ast.For) -> None:
        self.visit(node.iter)
        self.conditional_depth += 1
        try:
            self._bind_target(node.target, node.target)
            self._visit_conditional((*node.body, *node.orelse))
        finally:
            self.conditional_depth -= 1

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        self.visit(node.iter)
        self.conditional_depth += 1
        try:
            self._bind_target(node.target, node.target)
            self._visit_conditional((*node.body, *node.orelse))
        finally:
            self.conditional_depth -= 1

    def visit_With(self, node: ast.With) -> None:
        for item in node.items:
            self.visit(item.context_expr)
            self._bind_target(item.optional_vars, item)
        for statement in node.body:
            self.visit(statement)

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        for item in node.items:
            self.visit(item.context_expr)
            self._bind_target(item.optional_vars, item)
        for statement in node.body:
            self.visit(statement)

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        if node.type is not None:
            self.visit(node.type)
        if node.name:
            self.scope.bind(node.name, None, (int(node.lineno), int(node.col_offset)))
        for statement in node.body:
            self.visit(statement)

    def visit_match_case(self, node: ast.match_case) -> None:
        self.conditional_depth += 1
        try:
            for name in _binding_names(node.pattern):
                self.scope.bind(
                    name,
                    None,
                    _node_end(node.pattern),
                    conditional=True,
                )
            if node.guard is not None:
                self.visit(node.guard)
            for statement in node.body:
                self.visit(statement)
        finally:
            self.conditional_depth -= 1

    def _visit_function(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
    ) -> None:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                self.visit(decorator)
            if node.returns is not None:
                self.visit(node.returns)
        for default in (*node.args.defaults, *node.args.kw_defaults):
            if default is not None:
                self.visit(default)
        parent = self.scope
        child = _LexicalImportScope(parent)
        for name in _function_local_names(node):
            child.bind(name, None, (-1, -1))
        self.scope = child
        body = node.body if isinstance(node.body, list) else [node.body]
        for statement in body:
            self.visit(statement)
        self.scope = parent
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            parent.bind(node.name, None, _node_end(node))

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_function(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_function(node)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self._visit_function(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for expression in (*node.decorator_list, *node.bases):
            self.visit(expression)
        for keyword in node.keywords:
            self.visit(keyword.value)
        parent = self.scope
        self.scope = _LexicalImportScope(parent)
        for statement in node.body:
            self.visit(statement)
        self.scope = parent
        parent.bind(node.name, None, _node_end(node))

    def _visit_comprehension(self, node: ast.AST) -> None:
        parent = self.scope
        child = _LexicalImportScope(parent)
        generators = getattr(node, "generators")
        for generator in generators:
            for name in _binding_names(generator.target):
                child.bind(name, None, (-1, -1))
        self.scope = child
        self.generic_visit(node)
        self.scope = parent

    visit_ListComp = _visit_comprehension
    visit_SetComp = _visit_comprehension
    visit_DictComp = _visit_comprehension
    visit_GeneratorExp = _visit_comprehension

    def visit_Call(self, node: ast.Call) -> None:
        self.calls.append((node, self.scope))
        self.generic_visit(node)


def _dynamic_import_kind(
    call: ast.Call,
    scope: _LexicalImportScope,
) -> Literal["import_module", "dunder_import"] | None:
    position = (int(call.lineno), int(call.col_offset))
    callee = call.func
    if isinstance(callee, ast.Name):
        provenance = scope.resolve_all(callee.id, position)
        if "import_module" in provenance:
            return "import_module"
        if "dunder_import" in provenance:
            return "dunder_import"
        return None
    if (
        isinstance(callee, ast.Attribute)
        and callee.attr == "import_module"
        and isinstance(callee.value, ast.Name)
        and "importlib" in scope.resolve_all(callee.value.id, position)
    ):
        return "import_module"
    return None


def _dynamic_import_target(
    call: ast.Call,
    *,
    kind: Literal["import_module", "dunder_import"],
    contexts: tuple[ModuleImportContext, ...],
    path: Path,
) -> tuple[str, ...] | None:
    is_import_module = kind == "import_module"
    if kind not in {"import_module", "dunder_import"}:
        return None
    try:
        arguments = bind_static_import_call_arguments(call, kind)
    except ValueError as exc:
        raise ValueError(f"{exc} in {path}") from exc
    name_arg = arguments.name
    if not isinstance(name_arg, ast.Constant) or not isinstance(name_arg.value, str):
        raise ValueError(f"non-literal dynamic Python import in {path}")
    name = name_arg.value
    level = 0
    level_arg = arguments.level if not is_import_module else None
    if level_arg is not None:
        if isinstance(level_arg, ast.Constant) and isinstance(level_arg.value, int):
            level = level_arg.value
        elif (
            isinstance(level_arg, ast.UnaryOp)
            and isinstance(level_arg.op, ast.USub)
            and isinstance(level_arg.operand, ast.Constant)
            and isinstance(level_arg.operand.value, int)
        ):
            level = -level_arg.operand.value
        else:
            raise ValueError(f"non-literal __import__ level in {path}")
        if level < 0:
            raise ValueError(f"negative __import__ level in {path}")
    fromlist: list[str] = []
    fromlist_arg = arguments.fromlist if not is_import_module else None
    if fromlist_arg is not None:
        if not isinstance(fromlist_arg, (ast.Tuple, ast.List)):
            raise ValueError(f"non-literal __import__ fromlist in {path}")
        for item in fromlist_arg.elts:
            if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
                raise ValueError(f"non-literal __import__ fromlist in {path}")
            if item.value == "*":
                raise ValueError(
                    f"dynamic __import__ star fromlist requires a manifest in {path}"
                )
            fromlist.append(item.value)
    package_arg = arguments.package if is_import_module else None
    globals_arg = arguments.globals if not is_import_module else None
    modules: set[str] = set()
    errors: list[ValueError] = []
    for context in contexts:
        if is_import_module:
            request = StaticImportRequest.import_module(
                name,
                metadata_value_from_expression(package_arg, context),
            )
        else:
            request = StaticImportRequest(
                "dunder_import",
                name,
                level=level,
                fromlist=tuple(fromlist),
                globals_state=dunder_globals_state_from_expression(
                    globals_arg, context
                ),
                globals_were_supplied=globals_arg is not None,
            )
        try:
            modules.update(_project_import_request(request, context, path))
        except ValueError as exc:
            errors.append(exc)
    if errors:
        raise errors[0]
    if modules:
        return tuple(sorted(modules))
    return ()


def local_import_targets(
    path: Path,
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
    *,
    expected_nonliteral_dynamic_imports: int | None = None,
    nonliteral_dynamic_import_targets: tuple[str, ...] = (),
) -> set[str]:
    """Analyze one source into import targets according to ``policy``."""

    tree = resolver.read_ast(path)
    module, _package = resolver.module_identity(path)
    base_context = ModuleImportContext(
        module_name=module,
        is_package=path.name == "__init__.py",
        spec_name=module,
    )
    import_flow = analyze_module_import_flow(tree, base_context)

    def contexts_for(node: ast.AST) -> tuple[ModuleImportContext, ...]:
        return tuple(
            base_context.with_state(state) for state in import_flow.states_for(node)
        )
    nodes: list[ast.AST] = (
        list(tree.body) if policy.module_level_only else list(ast.walk(tree))
    )
    targets: set[str] = set()
    for node in nodes:
        if isinstance(node, ast.Import):
            targets.update(alias.name for alias in node.names)
            continue
        if not isinstance(node, ast.ImportFrom):
            continue
        # Keep unresolved fromlist candidates in the persistent graph.
        # Resolution applies the owner fallback against the live filesystem,
        # so adding ``pkg/name.py`` later changes the edge without reparsing.
        request = StaticImportRequest.statement(
            node.module or "",
            level=node.level,
            fromlist=tuple(alias.name for alias in node.names),
        )
        projected: set[str] = set()
        projection_errors: list[ValueError] = []
        for context in contexts_for(node):
            try:
                projected.update(_project_import_request(request, context, path))
            except ValueError as exc:
                projection_errors.append(exc)
        if projection_errors:
            raise projection_errors[0]
        targets.update(projected)

    nonliteral_dynamic_imports = 0
    if not policy.module_level_only:
        lexical_index = _DynamicImportLexicalIndex(tree)
        for node, scope in lexical_index.calls:
            kind = _dynamic_import_kind(node, scope)
            if kind is None:
                continue
            try:
                target = _dynamic_import_target(
                    node,
                    kind=kind,
                    contexts=contexts_for(node),
                    path=path,
                )
            except ValueError:
                nonliteral_dynamic_imports += 1
                if (
                    policy.fail_on_nonliteral_dynamic_import
                    and expected_nonliteral_dynamic_imports is None
                ):
                    raise
                continue
            if target is not None:
                targets.update(target)
        if (
            expected_nonliteral_dynamic_imports is not None
            and nonliteral_dynamic_imports != expected_nonliteral_dynamic_imports
        ):
            raise ValueError(
                f"dynamic Python import manifest drift in {path}: expected "
                f"{expected_nonliteral_dynamic_imports} non-literal calls, found "
                f"{nonliteral_dynamic_imports}"
            )
        targets.update(nonliteral_dynamic_import_targets)

    return targets


def resolve_local_import_targets(
    targets: set[str],
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
) -> set[Path]:
    """Resolve analyzed targets against the live local module topology."""

    dependencies: set[Path] = set()
    for target in targets:
        if policy.allowed_prefix is not None and not (
            target == policy.allowed_prefix
            or target.startswith(f"{policy.allowed_prefix}.")
        ):
            continue
        resolved = resolver.resolve_with_owner_fallback(target)
        if resolved is None:
            continue
        resolved_module, source = resolved
        if policy.include_parent_packages:
            dependencies.update(resolver.parent_package_sources(resolved_module))
        dependencies.add(source)
    return dependencies


def local_import_dependencies(
    path: Path,
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
    *,
    expected_nonliteral_dynamic_imports: int | None = None,
    nonliteral_dynamic_import_targets: tuple[str, ...] = (),
) -> set[Path]:
    """Analyze and resolve one source's local import edges."""

    targets = local_import_targets(
        path,
        resolver,
        policy,
        expected_nonliteral_dynamic_imports=expected_nonliteral_dynamic_imports,
        nonliteral_dynamic_import_targets=nonliteral_dynamic_import_targets,
    )
    return resolve_local_import_targets(targets, resolver, policy)
