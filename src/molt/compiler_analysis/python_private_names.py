"""Resolve class-private identifiers before lexical analysis and lowering.

The AST fields carry resolved bindings. Node-owned spelling records preserve
the source view for callable metadata, stringized annotations and source
identity. They are not a lookup registry or runtime name-dispatch mechanism.
The rules follow CPython's _Py_Mangle and _Py_MaybeMangle in symtable.c.
"""

from __future__ import annotations

import ast
import copy
from typing import Any

_SPELLINGS = "_molt_private_source_fields"
_IMPORT_BINDING = "_molt_private_import_binding"


def mangle_private_name(private: str | None, name: str) -> str:
    if not private or not name.startswith("__") or name.endswith("__") or "." in name:
        return name
    private = private.lstrip("_")
    return f"_{private}{name}" if private else name


def python_source_field(node: ast.AST, field: str, default: Any = None) -> Any:
    """Read source spelling without concealing a subsequent AST field edit."""
    current = getattr(node, field, default)
    pair = getattr(node, _SPELLINGS, {}).get(field)
    if pair is not None and current == pair[1]:
        return pair[0].copy() if isinstance(pair[0], list) else pair[0]
    return current


def python_definition_name(node: ast.AST) -> str:
    return python_source_field(node, "name")


def python_import_binding(alias: ast.alias) -> str:
    """Keep an implicit dotted import's root binding separate from its module."""
    admitted = getattr(alias, _IMPORT_BINDING, None)
    if admitted is not None and admitted[:2] == (alias.name, alias.asname):
        return admitted[2]
    return alias.asname or alias.name.partition(".")[0]


def python_source_unparse(node: ast.AST) -> str:
    class SourceView(ast.NodeTransformer):
        def generic_visit(self, node: ast.AST) -> ast.AST:
            result = copy.copy(node)
            for field, value in ast.iter_fields(node):
                value = python_source_field(node, field, value)
                if isinstance(value, ast.AST):
                    value = self.visit(value)
                elif isinstance(value, list):
                    value = [
                        self.visit(item) if isinstance(item, ast.AST) else item
                        for item in value
                    ]
                setattr(result, field, value)
            return result

    return ast.unparse(SourceView().visit(node))


class _PrivateNames(ast.NodeVisitor):
    def __init__(self) -> None:
        self.private: str | None = None
        self.type_parameters: set[str] | None = None

    def mangle(self, name: str) -> str:
        if self.type_parameters is not None and name not in self.type_parameters:
            return name
        return mangle_private_name(self.private, name)

    def field(self, node: ast.AST, field: str) -> None:
        # An already lowered fragment can be analyzed in a synthetic module.
        # Without its enclosing class syntax, preserve its resolved identity.
        if self.private is None:
            return
        source = python_source_field(node, field)
        if source is None:
            return
        value = (
            [self.mangle(name) for name in source]
            if isinstance(source, list)
            else self.mangle(source)
        )
        if value != source:
            spellings = getattr(node, _SPELLINGS, None)
            if spellings is None:
                spellings = {}
                setattr(node, _SPELLINGS, spellings)
            spellings[field] = (
                source.copy() if isinstance(source, list) else source,
                value.copy() if isinstance(value, list) else value,
            )
        else:
            getattr(node, _SPELLINGS, {}).pop(field, None)
        setattr(node, field, value)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        private = python_definition_name(node)
        self.field(node, "name")
        for decorator in node.decorator_list:
            self.visit(decorator)
        previous = self.private, self.type_parameters
        try:
            params = getattr(node, "type_params", ())
            if params:
                self.private = private
                self.type_parameters = {
                    python_definition_name(param) for param in params
                }
                self.type_parameter_bindings(params)
            for base in node.bases:
                self.visit(base)
            for keyword in node.keywords:
                self.visit(keyword)
            self.private, self.type_parameters = private, None
            for statement in node.body:
                self.visit(statement)
        finally:
            self.private, self.type_parameters = previous

    def visit_FunctionDef(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        self.field(node, "name")
        # Functions, lambdas and comprehensions retain the enclosing private
        # class context; only a nested class replaces it.
        for decorator in node.decorator_list:
            self.visit(decorator)
        for default in [*node.args.defaults, *node.args.kw_defaults]:
            if default is not None:
                self.visit(default)
        self.type_parameter_bindings(getattr(node, "type_params", ()))
        self.parameter_bindings(node.args)
        if node.returns is not None:
            self.visit(node.returns)
        for statement in node.body:
            self.visit(statement)

    visit_AsyncFunctionDef = visit_FunctionDef

    def parameter_bindings(self, node: ast.arguments) -> None:
        seen: set[str] = set()
        for arg in [
            *node.posonlyargs,
            *node.args,
            *node.kwonlyargs,
            node.vararg,
            node.kwarg,
        ]:
            if arg is None:
                continue
            self.visit(arg)
            if arg.arg in seen:
                raise SyntaxError(
                    f"duplicate argument '{arg.arg}' in function definition"
                )
            seen.add(arg.arg)

    def visit_arguments(self, node: ast.arguments) -> None:
        self.parameter_bindings(node)
        for default in [*node.defaults, *node.kw_defaults]:
            if default is not None:
                self.visit(default)

    def visit_Name(self, node: ast.Name) -> None:
        self.field(node, "id")

    def visit_Attribute(self, node: ast.Attribute) -> None:
        self.visit(node.value)
        self.field(node, "attr")

    def visit_arg(self, node: ast.arg) -> None:
        self.field(node, "arg")
        self.generic_visit(node)

    def visit_Global(self, node: ast.Global | ast.Nonlocal) -> None:
        self.field(node, "names")

    visit_Nonlocal = visit_Global

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        self.field(node, "name")
        self.generic_visit(node)

    def visit_MatchAs(self, node: ast.MatchAs | ast.MatchStar) -> None:
        self.field(node, "name")
        self.generic_visit(node)

    visit_MatchStar = visit_MatchAs

    def visit_MatchMapping(self, node: ast.MatchMapping) -> None:
        self.field(node, "rest")
        self.generic_visit(node)

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            previous = getattr(alias, _IMPORT_BINDING, None)
            if (
                self.private is None
                and previous is not None
                and previous[:2] == (alias.name, alias.asname)
            ):
                continue
            self.field(alias, "name")
            self.field(alias, "asname")
            binding = self.mangle(alias.asname or alias.name.partition(".")[0])
            if binding != (alias.asname or alias.name.partition(".")[0]):
                setattr(alias, _IMPORT_BINDING, (alias.name, alias.asname, binding))
            elif hasattr(alias, _IMPORT_BINDING):
                delattr(alias, _IMPORT_BINDING)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self.field(node, "module")
        for alias in node.names:
            self.field(alias, "name")
            self.field(alias, "asname")

    def visit_TypeVar(self, node: ast.AST) -> None:
        name = python_definition_name(node)
        if self.type_parameters is not None:
            self.type_parameters.add(name)
        self.field(node, "name")
        self.generic_visit(node)

    visit_ParamSpec = visit_TypeVar
    visit_TypeVarTuple = visit_TypeVar

    def type_parameter_bindings(
        self, parameters: list[ast.type_param] | tuple[()]
    ) -> None:
        seen: set[str] = set()
        for parameter in parameters:
            self.visit(parameter)
            if parameter.name in seen:
                raise SyntaxError(f"duplicate type parameter '{parameter.name}'")
            seen.add(parameter.name)

    def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
        self.visit(node.name)
        self.type_parameter_bindings(node.type_params)
        self.visit(node.value)


def resolve_python_private_names(tree: ast.Module) -> ast.Module:
    """Resolve a parsed generation in place, retaining node/source-site identity.

    No object-id or mutation-blind cache is kept. Re-entry is idempotent, and
    source spellings permit re-resolution after an intentional lexical move.
    Keyword argument labels, match-class labels and string constants stay raw.
    """
    if not isinstance(tree, ast.Module):
        raise TypeError("private-name resolution requires a complete module scope")
    _PrivateNames().visit(tree)
    return tree
