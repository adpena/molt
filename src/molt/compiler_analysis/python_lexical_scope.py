"""Lexical definition regions shared by binding and frontend projections.

Defaults belong to the enclosing scope even for generic functions. Definition
bodies and annotations have separate lexical custody. Consumers choose lexical
declarations, immediate name operations, or transitive closure dependencies;
the target Python policy is supplied explicitly, never inferred from the host.
"""

from __future__ import annotations

import ast
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from typing import Literal, TypeAlias

from molt.compiler_analysis.python_source_keys import python_pattern_capture_names

LexicalDefinitionNode: TypeAlias = (
    ast.FunctionDef
    | ast.AsyncFunctionDef
    | ast.Lambda
    | ast.ClassDef
    | ast.TypeAlias
    | ast.ListComp
    | ast.SetComp
    | ast.DictComp
    | ast.GeneratorExp
)


@dataclass(frozen=True, slots=True)
class DefinitionLexicalRegions:
    enclosing: tuple[ast.expr, ...]
    annotations: tuple[ast.expr, ...]
    body: tuple[ast.AST, ...]
    kind: Literal["function", "class", "annotation", "comprehension"]
    parameters: tuple[str, ...]
    type_parameters: frozenset[str]


def function_parameter_names(arguments: ast.arguments) -> tuple[str, ...]:
    return tuple(
        argument.arg
        for argument in (
            *arguments.posonlyargs,
            *arguments.args,
            *arguments.kwonlyargs,
            *((arguments.vararg,) if arguments.vararg is not None else ()),
            *((arguments.kwarg,) if arguments.kwarg is not None else ()),
        )
    )


def function_annotation_expressions(
    node: ast.FunctionDef | ast.AsyncFunctionDef,
) -> tuple[ast.expr, ...]:
    return (
        *(
            argument.annotation
            for argument in (
                *node.args.posonlyargs,
                *node.args.args,
                *((node.args.vararg,) if node.args.vararg is not None else ()),
                *node.args.kwonlyargs,
                *((node.args.kwarg,) if node.args.kwarg is not None else ()),
            )
            if argument.annotation is not None
        ),
        *((node.returns,) if node.returns is not None else ()),
    )


def type_parameter_expressions(parameters: Sequence[ast.AST]) -> tuple[ast.expr, ...]:
    return tuple(
        expression
        for parameter in parameters
        for attribute in ("bound", "default_value")
        if isinstance((expression := getattr(parameter, attribute, None)), ast.expr)
    )


def definition_lexical_regions(
    node: LexicalDefinitionNode,
    *,
    eager_annotations: bool,
    future_annotations: bool = False,
) -> DefinitionLexicalRegions:
    """Partition one definition without descending into any child definition."""
    if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
        expressions: list[ast.expr] = []
        for index, generator in enumerate(node.generators):
            if index:
                expressions.append(generator.iter)
            expressions.extend(generator.ifs)
        if isinstance(node, ast.DictComp):
            expressions.extend((node.key, node.value))
        else:
            expressions.append(node.elt)
        parameters = tuple(
            sorted(
                {
                    child.id
                    for generator in node.generators
                    for child in ast.walk(generator.target)
                    if isinstance(child, ast.Name) and isinstance(child.ctx, ast.Store)
                }
            )
        )
        return DefinitionLexicalRegions(
            (node.generators[0].iter,),
            (),
            tuple(expressions),
            "comprehension",
            parameters,
            frozenset(),
        )
    type_params = tuple(getattr(node, "type_params", ()))
    parameter_names = frozenset(parameter.name for parameter in type_params)
    annotations = list(type_parameter_expressions(type_params))
    enclosing: list[ast.expr] = []
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        enclosing.extend(node.decorator_list)
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
        enclosing.extend(node.args.defaults)
        enclosing.extend(value for value in node.args.kw_defaults if value is not None)
        if not isinstance(node, ast.Lambda) and not future_annotations:
            destination = (
                enclosing if eager_annotations and not type_params else annotations
            )
            destination.extend(function_annotation_expressions(node))
        body = (node.body,) if isinstance(node, ast.Lambda) else tuple(node.body)
        kind: Literal["function", "class", "annotation", "comprehension"] = "function"
        parameters = function_parameter_names(node.args)
    elif isinstance(node, ast.ClassDef):
        destination = annotations if type_params else enclosing
        destination.extend(node.bases)
        destination.extend(keyword.value for keyword in node.keywords)
        body = tuple(node.body)
        kind, parameters = "class", ()
    else:
        assert isinstance(node, ast.TypeAlias)
        annotations.append(node.value)
        body = ()
        kind, parameters = "annotation", ()
    return DefinitionLexicalRegions(
        tuple(enclosing), tuple(annotations), body, kind, parameters, parameter_names
    )


class PythonLexicalScopeVisitor(ast.NodeVisitor):
    """Visit enclosing lexical regions; never cross a definition body.

    Variable annotations participate in eager lexical declarations even inside
    functions, where their expressions are not executed. Runtime projections
    disable that visit explicitly for function-local annotations.
    """

    def __init__(
        self,
        *,
        eager_annotations: bool,
        variable_annotations: bool | None = None,
    ) -> None:
        self.eager_annotations = eager_annotations
        self.variable_annotations = (
            eager_annotations if variable_annotations is None else variable_annotations
        )

    def _visit_definition_header(
        self,
        node: LexicalDefinitionNode,
    ) -> None:
        regions = definition_lexical_regions(
            node, eager_annotations=self.eager_annotations
        )
        for expression in regions.enclosing:
            self.visit(expression)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_definition_header(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_definition_header(node)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self._visit_definition_header(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self._visit_definition_header(node)

    def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
        self._visit_definition_header(node)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        self.visit(node.target)
        if node.value is not None:
            self.visit(node.value)
        if self.variable_annotations:
            self.visit(node.annotation)


def class_body_functions(
    node: ast.ClassDef,
) -> tuple[ast.FunctionDef | ast.AsyncFunctionDef, ...]:
    """Method definitions in source order, across blocks but not child scopes."""
    functions: list[ast.FunctionDef | ast.AsyncFunctionDef] = []

    class Collector(PythonLexicalScopeVisitor):
        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            functions.append(node)

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            functions.append(node)

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            return

    collector = Collector(eager_annotations=False)
    for statement in node.body:
        collector.visit(statement)
    return tuple(functions)


def class_annotation_syntax_error(
    node: ast.Module, *, target_python: tuple[int, int], future_annotations: bool
) -> tuple[ast.AST, str] | None:
    """CPython 3.12 forbids nested code in a class-visible annotation scope.

    This is a syntax property, including unreachable statements. Definition
    regions distinguish generic annotations/bounds/bases from ordinary eager
    annotations and defaults; function bodies stop class visibility.
    """
    if target_python >= (3, 13):
        return None

    class Validator(PythonLexicalScopeVisitor):
        class_visible = False
        annotation_scope = False
        error: tuple[ast.AST, str] | None = None

        def visit(self, node: ast.AST) -> None:
            if self.error is None:
                super().visit(node)

        def _region(
            self, children: Sequence[ast.AST], *, class_visible: bool, annotation: bool
        ) -> None:
            saved = self.class_visible, self.annotation_scope
            self.class_visible, self.annotation_scope = class_visible, annotation
            for child in children:
                self.visit(child)
            self.class_visible, self.annotation_scope = saved

        def _visit_definition_header(self, node: LexicalDefinitionNode) -> None:
            if (
                self.class_visible
                and self.annotation_scope
                and isinstance(
                    node,
                    (
                        ast.Lambda,
                        ast.ListComp,
                        ast.SetComp,
                        ast.DictComp,
                        ast.GeneratorExp,
                    ),
                )
            ):
                kind = "lambda" if isinstance(node, ast.Lambda) else "comprehension"
                self.error = (
                    node,
                    f"Cannot use {kind} in annotation scope within class scope",
                )
                return
            regions = definition_lexical_regions(
                node, eager_annotations=True, future_annotations=future_annotations
            )
            for expression in regions.enclosing:
                self.visit(expression)
            self._region(
                regions.annotations, class_visible=self.class_visible, annotation=True
            )
            self._region(
                regions.body, class_visible=regions.kind == "class", annotation=False
            )

        def visit_ListComp(self, node: ast.ListComp) -> None:
            self._visit_definition_header(node)

        def visit_SetComp(self, node: ast.SetComp) -> None:
            self._visit_definition_header(node)

        def visit_DictComp(self, node: ast.DictComp) -> None:
            self._visit_definition_header(node)

        def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
            self._visit_definition_header(node)

    validator = Validator(eager_annotations=not future_annotations)
    validator.visit(node)
    return validator.error


class ScopedNamedExprCollector(PythonLexicalScopeVisitor):
    """Project walrus writes, retaining source order and repeated writes."""

    def __init__(
        self, record: Callable[[str], None], *, eager_annotations: bool
    ) -> None:
        super().__init__(eager_annotations=eager_annotations)
        self.record = record

    def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
        if isinstance(node.target, ast.Name):
            self.record(node.target.id)
        self.visit(node.value)


@dataclass(frozen=True, slots=True)
class PythonDependencyProjection:
    """Direct reads and transitive captures remain distinct for storage planning."""

    direct: frozenset[str]
    nested: frozenset[str]
    globals: frozenset[str]

    @property
    def lexical(self) -> frozenset[str]:
        return self.direct | self.nested


@dataclass(frozen=True, slots=True)
class PythonScopeDeclarations:
    bound: frozenset[str]
    globals: frozenset[str]
    nonlocals: frozenset[str]


class _DeclarationCollector(PythonLexicalScopeVisitor):
    """One scope-local symbol-table pass; nested lexical scopes are skipped."""

    def __init__(self, *, eager_annotations: bool) -> None:
        super().__init__(eager_annotations=eager_annotations)
        self.bound: set[str] = set()
        self.globals: set[str] = set()
        self.nonlocals: set[str] = set()

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, (ast.Store, ast.Del)):
            self.bound.add(node.id)

    def visit_Import(self, node: ast.Import) -> None:
        self.bound.update(
            alias.asname or alias.name.split(".", 1)[0] for alias in node.names
        )

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self.bound.update(
            alias.asname or alias.name for alias in node.names if alias.name != "*"
        )

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.bound.add(node.name)
        super().visit_FunctionDef(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.bound.add(node.name)
        super().visit_AsyncFunctionDef(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.bound.add(node.name)
        super().visit_ClassDef(node)

    def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
        self.visit(node.name)

    def visit_Global(self, node: ast.Global) -> None:
        self.globals.update(node.names)

    def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
        self.nonlocals.update(node.names)

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        if node.name is not None:
            self.bound.add(node.name)
        self.generic_visit(node)

    def visit_Match(self, node: ast.Match) -> None:
        for case in node.cases:
            self.bound.update(python_pattern_capture_names(case.pattern))
        self.generic_visit(node)

    def visit_comprehension(self, node: ast.comprehension) -> None:
        self.visit(node.iter)
        for condition in node.ifs:
            self.visit(condition)


@dataclass(frozen=True, slots=True)
class PythonLexicalDependencies:
    lexical: frozenset[str]
    globals: frozenset[str]


@dataclass(frozen=True, slots=True)
class PythonDefinitionDependencies:
    body: PythonLexicalDependencies
    annotations: PythonLexicalDependencies
    class_cell_required: bool = False


class _DependencyProjection(PythonLexicalScopeVisitor):
    """Collect direct loads and already summarized nested lexical regions."""

    def __init__(
        self,
        authority: PythonDependencyAuthority,
        *,
        variable_annotations: bool,
        deferred_variable_annotations: bool = False,
    ) -> None:
        super().__init__(
            eager_annotations=authority.eager_annotations,
            variable_annotations=variable_annotations,
        )
        self.authority = authority
        self.deferred_variable_annotations = deferred_variable_annotations
        self.loads: set[str] = set()
        self.loads_class_cell = False
        self.loads_class_name = False
        self.nested: set[str] = set()
        self.globals: set[str] = set()

    def visit(self, node: ast.AST) -> None:
        self.authority.node_visits += 1
        super().visit(node)

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, ast.Load) and node.id in {"super", "__class__"}:
            # CPython's implicit cell rule is syntactic, including a shadowed
            # super. Record it before value/global lookup filters erase the load.
            self.loads_class_cell = True
            self.loads_class_name |= node.id == "__class__"
        if isinstance(node.ctx, ast.Load) and (
            self.authority.include_lexical_read is None
            or self.authority.include_lexical_read(node)
        ):
            self.loads.add(node.id)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        super().visit_AnnAssign(node)
        if self.deferred_variable_annotations and node.simple:
            annotation = self.authority.project(
                (node.annotation,), implicit_class_cell=True
            )
            self.nested.update(annotation.lexical)
            self.globals.update(annotation.globals)

    def _visit_definition_header(self, node: LexicalDefinitionNode) -> None:
        regions = self.authority.regions(node)
        for expression in regions.enclosing:
            self.visit(expression)
        summary = self.authority.summary(node)
        self.nested.update(
            (summary.body.lexical | summary.annotations.lexical)
            - regions.type_parameters
        )
        self.globals.update(summary.body.globals | summary.annotations.globals)

    def visit_ListComp(self, node: ast.ListComp) -> None:
        self._visit_definition_header(node)

    def visit_SetComp(self, node: ast.SetComp) -> None:
        self._visit_definition_header(node)

    def visit_DictComp(self, node: ast.DictComp) -> None:
        self._visit_definition_header(node)

    def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
        self._visit_definition_header(node)


class PythonDependencyAuthority:
    """Memoized transitive free/global dependencies, one summary per AST scope.

    Function locals suppress nested free dependencies; class locals do not.
    Explicit global references remain separate when a sibling captures the same
    spelling lexically. Header regions are scanned in their enclosing scope,
    while generic/deferred annotations retain their own type-parameter custody.
    """

    def __init__(
        self,
        *,
        eager_annotations: bool,
        future_annotations: bool,
        include_lexical_read: Callable[[ast.Name], bool] | None = None,
    ) -> None:
        self.eager_annotations = eager_annotations
        self.future_annotations = future_annotations
        self.include_lexical_read = include_lexical_read
        self.node_visits = 0
        self.declaration_scans = 0
        self._declarations: dict[ast.AST, PythonScopeDeclarations] = {}
        self.summaries: dict[ast.AST, PythonDefinitionDependencies] = {}
        self._regions: dict[ast.AST, DefinitionLexicalRegions] = {}

    def project(
        self,
        nodes: Sequence[ast.AST],
        *,
        variable_annotations: bool = False,
        implicit_class_cell: bool = False,
    ) -> PythonDependencyProjection:
        visitor = _DependencyProjection(self, variable_annotations=variable_annotations)
        for node in nodes:
            visitor.visit(node)
        if implicit_class_cell and visitor.loads_class_cell:
            visitor.loads.add("__class__")
        return PythonDependencyProjection(
            frozenset(visitor.loads),
            frozenset(visitor.nested),
            frozenset(visitor.globals),
        )

    def regions(self, node: LexicalDefinitionNode) -> DefinitionLexicalRegions:
        result = self._regions.get(node)
        if result is None:
            result = definition_lexical_regions(
                node,
                eager_annotations=self.eager_annotations,
                future_annotations=self.future_annotations,
            )
            self._regions[node] = result
        return result

    def declarations(self, node: LexicalDefinitionNode) -> PythonScopeDeclarations:
        cached = self._declarations.get(node)
        if cached is not None:
            return cached
        regions = self.regions(node)
        self.declaration_scans += 1
        if regions.kind == "comprehension":
            # Walrus targets in a comprehension belong to its enclosing scope.
            nonlocals: set[str] = set()
            collector = ScopedNamedExprCollector(
                nonlocals.add,
                eager_annotations=self.eager_annotations
                and not self.future_annotations,
            )
            for expression in regions.body:
                collector.visit(expression)
            declarations = PythonScopeDeclarations(
                frozenset(regions.parameters), frozenset(), frozenset(nonlocals)
            )
        else:
            declaration_body = [
                statement
                if isinstance(statement, ast.stmt)
                else ast.Expr(value=statement)
                for statement in regions.body
                if isinstance(statement, (ast.stmt, ast.expr))
            ]
            declarations = python_scope_declarations(
                declaration_body,
                regions.parameters,
                eager_annotations=self.eager_annotations
                and not self.future_annotations,
            )
        self._declarations[node] = declarations
        return declarations

    def summary(self, node: LexicalDefinitionNode) -> PythonDefinitionDependencies:
        cached = self.summaries.get(node)
        if cached is not None:
            return cached
        regions = self.regions(node)
        declarations = self.declarations(node)
        body = _DependencyProjection(
            self,
            variable_annotations=(
                self.eager_annotations
                and not self.future_annotations
                and regions.kind == "class"
            ),
            deferred_variable_annotations=(
                not self.eager_annotations
                and not self.future_annotations
                and regions.kind == "class"
            ),
        )
        for statement in regions.body:
            body.visit(statement)
        if regions.kind == "class":
            lexical = (
                (
                    (body.loads | ({"__class__"} if body.loads_class_name else set()))
                    - declarations.bound
                    - declarations.globals
                )
                | (body.nested - {"__class__"})
                | declarations.nonlocals
            )
            global_names = body.globals | (
                body.loads & (declarations.bound | declarations.globals)
            )
        else:
            names = body.loads | body.nested | declarations.nonlocals
            if regions.kind in {"function", "comprehension"} and body.loads_class_cell:
                names.add("__class__")
            lexical = names - declarations.bound - declarations.globals
            global_names = body.globals | (names & declarations.globals)
        annotations = self.project(regions.annotations, implicit_class_cell=True)
        result = PythonDefinitionDependencies(
            PythonLexicalDependencies(frozenset(lexical), frozenset(global_names)),
            PythonLexicalDependencies(
                annotations.lexical,
                frozenset(annotations.globals),
            ),
            class_cell_required=(
                regions.kind == "class" and "__class__" in body.nested
            ),
        )
        self.summaries[node] = result
        return result


def python_scope_declarations(
    body: Sequence[ast.stmt],
    parameters: Iterable[str] = (),
    *,
    eager_annotations: bool,
) -> PythonScopeDeclarations:
    collector = _DeclarationCollector(eager_annotations=eager_annotations)
    for statement in body:
        collector.visit(statement)
    bound = (
        (collector.bound | set(parameters)) - collector.globals - collector.nonlocals
    )
    return PythonScopeDeclarations(
        frozenset(bound), frozenset(collector.globals), frozenset(collector.nonlocals)
    )
