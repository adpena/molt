"""Top-level function-metadata analysis (doc 44 §F2b: "function metadata —
param counts / defaults shapes / generator-vs-async classification").

Free functions over ``ast`` nodes — the ``cfg_analysis.py`` house shape.  Lifts
``SimpleTIRGenerator._collect_module_func_kinds`` /
``_collect_module_class_names`` / ``_collect_module_func_defaults`` and their
pure dependencies (``_function_contains_yield``, ``_function_param_names``,
``_split_function_args``, ``_default_specs_from_args``,
``_default_spec_for_expr``) verbatim.  These are pure functions of the AST today;
``self`` was used only to call other pure helpers.

The defaults table here is the AST-derived value.  The walk prefers an
externally-supplied ``known_func_defaults`` override when present; that override
is applied by the populate-shim (it is a runtime input, not an AST fact), so it
deliberately does **not** live in this module.
"""

from __future__ import annotations

import ast
from typing import Any


def _function_contains_yield(
    node: ast.FunctionDef | ast.AsyncFunctionDef,
) -> bool:
    def push_arg_annotations(stack: list[ast.AST], args: ast.arguments) -> None:
        for arg in (
            args.posonlyargs
            + args.args
            + args.kwonlyargs
            + ([] if args.vararg is None else [args.vararg])
            + ([] if args.kwarg is None else [args.kwarg])
        ):
            if arg.annotation is not None:
                stack.append(arg.annotation)

    stack: list[ast.AST] = list(node.body)
    while stack:
        current = stack.pop()
        if isinstance(current, (ast.Yield, ast.YieldFrom)):
            return True
        if isinstance(current, (ast.FunctionDef, ast.AsyncFunctionDef)):
            stack.extend(current.decorator_list)
            stack.extend(current.args.defaults)
            stack.extend(
                default for default in current.args.kw_defaults if default is not None
            )
            push_arg_annotations(stack, current.args)
            if current.returns is not None:
                stack.append(current.returns)
            continue
        if isinstance(current, ast.ClassDef):
            stack.extend(current.decorator_list)
            stack.extend(current.bases)
            stack.extend(keyword.value for keyword in current.keywords)
            continue
        if isinstance(current, ast.Lambda):
            continue
        stack.extend(ast.iter_child_nodes(current))
    return False


def _split_function_args(
    args: ast.arguments,
) -> tuple[list[ast.arg], list[ast.arg], list[ast.arg], str | None, str | None]:
    posonly = list(args.posonlyargs)
    pos_or_kw = list(args.args)
    kwonly = list(args.kwonlyargs)
    vararg = args.vararg.arg if args.vararg else None
    varkw = args.kwarg.arg if args.kwarg else None
    return posonly, pos_or_kw, kwonly, vararg, varkw


def _function_param_names(args: ast.arguments) -> list[str]:
    posonly, pos_or_kw, kwonly, vararg, varkw = _split_function_args(args)
    names = [arg.arg for arg in posonly + pos_or_kw]
    if vararg is not None:
        names.append(vararg)
    names.extend(arg.arg for arg in kwonly)
    if varkw is not None:
        names.append(varkw)
    return names


def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
    if isinstance(expr, ast.Constant):
        return {"const": True, "value": expr.value}
    return {"const": False}


def _default_specs_from_args(args: ast.arguments) -> list[dict[str, Any]]:
    default_specs = [_default_spec_for_expr(expr) for expr in args.defaults]
    if not args.kwonlyargs or not args.kw_defaults:
        return default_specs
    kwonly_names = [arg.arg for arg in args.kwonlyargs]
    kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
    suffix: list[tuple[str, ast.expr]] = []
    for name, expr in reversed(kwonly_pairs):
        if expr is None:
            break
        suffix.append((name, expr))
    for name, expr in reversed(suffix):
        spec = _default_spec_for_expr(expr)
        spec["kwonly"] = True
        spec["name"] = name
        default_specs.append(spec)
    return default_specs


def collect_module_func_kinds(node: ast.Module) -> dict[str, str]:
    kinds: dict[str, str] = {}
    for stmt in node.body:
        if isinstance(stmt, ast.AsyncFunctionDef):
            kinds[stmt.name] = "asyncgen" if _function_contains_yield(stmt) else "async"
        elif isinstance(stmt, ast.FunctionDef):
            if _function_contains_yield(stmt):
                kinds[stmt.name] = "gen"
            else:
                kinds[stmt.name] = "sync"
    return kinds


def collect_module_class_names(node: ast.Module) -> set[str]:
    return {stmt.name for stmt in node.body if isinstance(stmt, ast.ClassDef)}


def collect_module_func_defaults(node: ast.Module) -> dict[str, dict[str, Any]]:
    defaults: dict[str, dict[str, Any]] = {}
    for stmt in node.body:
        if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if isinstance(stmt, ast.AsyncFunctionDef):
            kind = "asyncgen" if _function_contains_yield(stmt) else "async"
        else:
            kind = "gen" if _function_contains_yield(stmt) else "sync"
        has_decorators = bool(stmt.decorator_list)
        if stmt.args.vararg or stmt.args.kwarg:
            defaults[stmt.name] = {
                "has_vararg": True,
                "kind": kind,
                "has_decorators": has_decorators,
            }
            continue
        params = _function_param_names(stmt.args)
        default_specs = _default_specs_from_args(stmt.args)
        defaults[stmt.name] = {
            "params": len(params),
            "defaults": default_specs,
            "posonly": len(stmt.args.posonlyargs),
            "kwonly": len(stmt.args.kwonlyargs),
            "kind": kind,
            "has_decorators": has_decorators,
        }
    return defaults
