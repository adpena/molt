#!/usr/bin/env python3
"""Differential test coverage analysis tool.

Parses all differential test files using the AST to extract Python features
exercised, cross-references against compatibility surface matrices, identifies
coverage gaps, and generates structured reports.

Usage:
    python3 tools/diff_coverage_analysis.py
    python3 tools/diff_coverage_analysis.py --json
    python3 tools/diff_coverage_analysis.py --markdown
    python3 tools/diff_coverage_analysis.py --gaps-only
    python3 tools/diff_coverage_analysis.py --category operators
"""

from __future__ import annotations

import argparse
import ast
import json
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIFF_ROOT = ROOT / "tests" / "differential"

# ---------------------------------------------------------------------------
# Feature taxonomy
# ---------------------------------------------------------------------------

FEATURE_CATEGORIES: dict[str, list[str]] = {
    "control_flow": [
        "if_statement",
        "if_else",
        "elif",
        "for_loop",
        "for_else",
        "while_loop",
        "while_else",
        "break",
        "continue",
        "pass",
        "match_statement",
        "return",
        "yield",
        "yield_from",
        "raise",
        "assert",
    ],
    "exception_handling": [
        "try_except",
        "try_finally",
        "try_else",
        "except_as",
        "except_star",
        "bare_except",
        "raise_from",
        "raise_bare",
    ],
    "functions": [
        "function_def",
        "lambda",
        "default_args",
        "star_args",
        "star_kwargs",
        "keyword_only_args",
        "positional_only_args",
        "decorator",
        "nested_function",
        "recursive_function",
        "closure",
        "global_keyword",
        "nonlocal_keyword",
    ],
    "classes": [
        "class_def",
        "inheritance",
        "multiple_inheritance",
        "class_decorator",
        "staticmethod",
        "classmethod",
        "property",
        "dunder_init",
        "dunder_repr",
        "dunder_str",
        "dunder_eq",
        "dunder_hash",
        "dunder_lt",
        "dunder_le",
        "dunder_gt",
        "dunder_ge",
        "dunder_add",
        "dunder_sub",
        "dunder_mul",
        "dunder_truediv",
        "dunder_floordiv",
        "dunder_mod",
        "dunder_pow",
        "dunder_neg",
        "dunder_len",
        "dunder_getitem",
        "dunder_setitem",
        "dunder_delitem",
        "dunder_contains",
        "dunder_iter",
        "dunder_next",
        "dunder_call",
        "dunder_enter",
        "dunder_exit",
        "dunder_del",
        "dunder_bool",
        "dunder_getattr",
        "dunder_setattr",
        "dunder_delattr",
        "slots",
        "super_call",
    ],
    "async": [
        "async_def",
        "await_expr",
        "async_for",
        "async_with",
        "async_comprehension",
        "async_generator",
    ],
    "comprehensions": [
        "list_comprehension",
        "dict_comprehension",
        "set_comprehension",
        "generator_expression",
        "nested_comprehension",
        "comprehension_filter",
    ],
    "operators": [
        "add",
        "sub",
        "mult",
        "div",
        "floordiv",
        "mod",
        "pow_op",
        "matmul",
        "lshift",
        "rshift",
        "bitor",
        "bitxor",
        "bitand",
        "invert",
        "unary_neg",
        "unary_pos",
        "not_op",
        "and_op",
        "or_op",
        "eq",
        "noteq",
        "lt",
        "lte",
        "gt",
        "gte",
        "is_op",
        "is_not",
        "in_op",
        "not_in",
        "augmented_assign",
        "walrus",
    ],
    "builtin_functions": [
        "print",
        "len",
        "range",
        "type",
        "isinstance",
        "issubclass",
        "int_constructor",
        "float_constructor",
        "str_constructor",
        "bool_constructor",
        "list_constructor",
        "tuple_constructor",
        "dict_constructor",
        "set_constructor",
        "frozenset_constructor",
        "bytes_constructor",
        "bytearray_constructor",
        "enumerate",
        "zip",
        "map",
        "filter",
        "sorted",
        "reversed",
        "iter_builtin",
        "next_builtin",
        "abs",
        "min",
        "max",
        "sum",
        "round",
        "pow_builtin",
        "divmod",
        "hash_builtin",
        "id_builtin",
        "repr_builtin",
        "chr_builtin",
        "ord_builtin",
        "hex_builtin",
        "oct_builtin",
        "bin_builtin",
        "all_builtin",
        "any_builtin",
        "callable_builtin",
        "getattr_builtin",
        "setattr_builtin",
        "delattr_builtin",
        "hasattr_builtin",
        "format_builtin",
        "ascii_builtin",
        "input_builtin",
        "open_builtin",
        "super_builtin",
        "vars_builtin",
        "dir_builtin",
        "globals_builtin",
        "locals_builtin",
        "memoryview_constructor",
        "complex_constructor",
        "slice_constructor",
        "object_constructor",
        "property_builtin",
        "classmethod_builtin",
        "staticmethod_builtin",
        "breakpoint_builtin",
    ],
    "string_operations": [
        "fstring",
        "fstring_expression",
        "fstring_format_spec",
        "fstring_debug",
        "fstring_conversion",
        "string_concat",
        "string_repeat",
        "string_slice",
        "string_method_call",
        "bytes_literal",
        "raw_string",
        "multiline_string",
    ],
    "collection_operations": [
        "list_literal",
        "tuple_literal",
        "dict_literal",
        "set_literal",
        "subscript_access",
        "slice_access",
        "list_method_call",
        "dict_method_call",
        "set_method_call",
        "unpacking_assign",
        "starred_unpack",
        "dict_unpack",
    ],
    "imports": [
        "import_module",
        "import_from",
        "import_alias",
        "import_star",
        "relative_import",
    ],
    "context_managers": [
        "with_statement",
        "with_as",
        "nested_with",
    ],
    "type_hints": [
        "function_annotation",
        "variable_annotation",
        "type_alias",
        "generic_class",
    ],
}

# Mapping from builtin function names to feature keys.
_BUILTIN_FUNCTION_MAP: dict[str, str] = {
    "print": "print",
    "len": "len",
    "range": "range",
    "type": "type",
    "isinstance": "isinstance",
    "issubclass": "issubclass",
    "int": "int_constructor",
    "float": "float_constructor",
    "str": "str_constructor",
    "bool": "bool_constructor",
    "list": "list_constructor",
    "tuple": "tuple_constructor",
    "dict": "dict_constructor",
    "set": "set_constructor",
    "frozenset": "frozenset_constructor",
    "bytes": "bytes_constructor",
    "bytearray": "bytearray_constructor",
    "enumerate": "enumerate",
    "zip": "zip",
    "map": "map",
    "filter": "filter",
    "sorted": "sorted",
    "reversed": "reversed",
    "iter": "iter_builtin",
    "next": "next_builtin",
    "abs": "abs",
    "min": "min",
    "max": "max",
    "sum": "sum",
    "round": "round",
    "pow": "pow_builtin",
    "divmod": "divmod",
    "hash": "hash_builtin",
    "id": "id_builtin",
    "repr": "repr_builtin",
    "chr": "chr_builtin",
    "ord": "ord_builtin",
    "hex": "hex_builtin",
    "oct": "oct_builtin",
    "bin": "bin_builtin",
    "all": "all_builtin",
    "any": "any_builtin",
    "callable": "callable_builtin",
    "getattr": "getattr_builtin",
    "setattr": "setattr_builtin",
    "delattr": "delattr_builtin",
    "hasattr": "hasattr_builtin",
    "format": "format_builtin",
    "ascii": "ascii_builtin",
    "input": "input_builtin",
    "open": "open_builtin",
    "super": "super_builtin",
    "vars": "vars_builtin",
    "dir": "dir_builtin",
    "globals": "globals_builtin",
    "locals": "locals_builtin",
    "memoryview": "memoryview_constructor",
    "complex": "complex_constructor",
    "slice": "slice_constructor",
    "object": "object_constructor",
    "property": "property_builtin",
    "classmethod": "classmethod_builtin",
    "staticmethod": "staticmethod_builtin",
    "breakpoint": "breakpoint_builtin",
}

# Dunder method names to feature keys.
_DUNDER_MAP: dict[str, str] = {
    "__init__": "dunder_init",
    "__repr__": "dunder_repr",
    "__str__": "dunder_str",
    "__eq__": "dunder_eq",
    "__hash__": "dunder_hash",
    "__lt__": "dunder_lt",
    "__le__": "dunder_le",
    "__gt__": "dunder_gt",
    "__ge__": "dunder_ge",
    "__add__": "dunder_add",
    "__radd__": "dunder_add",
    "__sub__": "dunder_sub",
    "__rsub__": "dunder_sub",
    "__mul__": "dunder_mul",
    "__rmul__": "dunder_mul",
    "__truediv__": "dunder_truediv",
    "__rtruediv__": "dunder_truediv",
    "__floordiv__": "dunder_floordiv",
    "__rfloordiv__": "dunder_floordiv",
    "__mod__": "dunder_mod",
    "__rmod__": "dunder_mod",
    "__pow__": "dunder_pow",
    "__rpow__": "dunder_pow",
    "__neg__": "dunder_neg",
    "__len__": "dunder_len",
    "__getitem__": "dunder_getitem",
    "__setitem__": "dunder_setitem",
    "__delitem__": "dunder_delitem",
    "__contains__": "dunder_contains",
    "__iter__": "dunder_iter",
    "__next__": "dunder_next",
    "__call__": "dunder_call",
    "__enter__": "dunder_enter",
    "__exit__": "dunder_exit",
    "__del__": "dunder_del",
    "__bool__": "dunder_bool",
    "__getattr__": "dunder_getattr",
    "__setattr__": "dunder_setattr",
    "__delattr__": "dunder_delattr",
}


# ---------------------------------------------------------------------------
# AST feature extractor
# ---------------------------------------------------------------------------


@dataclass
class FileFeatures:
    """Features extracted from a single test file."""

    path: str
    features: set[str] = field(default_factory=set)
    error: str | None = None


def _is_string_node(node: ast.AST) -> bool:
    """Heuristic: check if a node is likely a string value."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return True
    if isinstance(node, ast.JoinedStr):
        return True
    return False


class FeatureExtractor(ast.NodeVisitor):
    """Walk a Python AST and record which features are exercised."""

    def __init__(self) -> None:
        self.features: set[str] = set()
        self._in_class: bool = False
        self._class_depth: int = 0

    # -- Control flow -------------------------------------------------------

    def visit_If(self, node: ast.If) -> None:
        self.features.add("if_statement")
        if node.orelse:
            # Distinguish elif from else.
            if len(node.orelse) == 1 and isinstance(node.orelse[0], ast.If):
                self.features.add("elif")
            else:
                self.features.add("if_else")
        self.generic_visit(node)

    def visit_For(self, node: ast.For) -> None:
        self.features.add("for_loop")
        if node.orelse:
            self.features.add("for_else")
        self.generic_visit(node)

    def visit_While(self, node: ast.While) -> None:
        self.features.add("while_loop")
        if node.orelse:
            self.features.add("while_else")
        self.generic_visit(node)

    def visit_Break(self, node: ast.Break) -> None:
        self.features.add("break")
        self.generic_visit(node)

    def visit_Continue(self, node: ast.Continue) -> None:
        self.features.add("continue")
        self.generic_visit(node)

    def visit_Pass(self, node: ast.Pass) -> None:
        self.features.add("pass")
        self.generic_visit(node)

    def visit_Match(self, node: ast.Match) -> None:
        self.features.add("match_statement")
        self.generic_visit(node)

    def visit_Return(self, node: ast.Return) -> None:
        self.features.add("return")
        self.generic_visit(node)

    def visit_Yield(self, node: ast.Yield) -> None:
        self.features.add("yield")
        self.generic_visit(node)

    def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
        self.features.add("yield_from")
        self.generic_visit(node)

    def visit_Raise(self, node: ast.Raise) -> None:
        self.features.add("raise")
        if node.cause:
            self.features.add("raise_from")
        if node.exc is None:
            self.features.add("raise_bare")
        self.generic_visit(node)

    def visit_Assert(self, node: ast.Assert) -> None:
        self.features.add("assert")
        self.generic_visit(node)

    # -- Exception handling -------------------------------------------------

    def visit_Try(self, node: ast.Try) -> None:
        if node.handlers:
            self.features.add("try_except")
        if node.finalbody:
            self.features.add("try_finally")
        if node.orelse:
            self.features.add("try_else")
        for handler in node.handlers:
            if handler.name:
                self.features.add("except_as")
            if handler.type is None:
                self.features.add("bare_except")
        self.generic_visit(node)

    def visit_TryStar(self, node: ast.TryStar) -> None:
        self.features.add("except_star")
        self.generic_visit(node)

    # -- Functions ----------------------------------------------------------

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.features.add("function_def")
        self._analyze_function(node)
        self.generic_visit(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.features.add("async_def")
        self.features.add("function_def")
        self._analyze_function(node)
        # Check body for async generator (yield inside async def).
        for child in ast.walk(node):
            if isinstance(child, ast.Yield | ast.YieldFrom):
                self.features.add("async_generator")
                break
        self.generic_visit(node)

    def _analyze_function(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        args = node.args
        if args.defaults or args.kw_defaults:
            self.features.add("default_args")
        if args.vararg:
            self.features.add("star_args")
        if args.kwarg:
            self.features.add("star_kwargs")
        if args.kwonlyargs:
            self.features.add("keyword_only_args")
        if args.posonlyargs:
            self.features.add("positional_only_args")
        if node.decorator_list:
            self.features.add("decorator")
            if self._in_class:
                self.features.add("class_decorator")
            for dec in node.decorator_list:
                if isinstance(dec, ast.Name):
                    if dec.id == "staticmethod":
                        self.features.add("staticmethod")
                    elif dec.id == "classmethod":
                        self.features.add("classmethod")
                    elif dec.id == "property":
                        self.features.add("property")
                elif isinstance(dec, ast.Attribute):
                    if dec.attr in ("setter", "getter", "deleter"):
                        self.features.add("property")
        # Return annotations.
        if node.returns:
            self.features.add("function_annotation")
        for arg in args.args + args.posonlyargs + args.kwonlyargs:
            if arg.annotation:
                self.features.add("function_annotation")
                break

        # Check for nested functions.
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.FunctionDef | ast.AsyncFunctionDef):
                self.features.add("nested_function")
                break

        # Check for dunder methods inside classes.
        if self._in_class and node.name in _DUNDER_MAP:
            self.features.add(_DUNDER_MAP[node.name])

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self.features.add("lambda")
        self.generic_visit(node)

    # -- Classes ------------------------------------------------------------

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.features.add("class_def")
        if node.bases:
            self.features.add("inheritance")
            if len(node.bases) > 1:
                self.features.add("multiple_inheritance")
        if node.decorator_list:
            self.features.add("class_decorator")
            self.features.add("decorator")

        # Check for __slots__.
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.Assign):
                for target in child.targets:
                    if isinstance(target, ast.Name) and target.id == "__slots__":
                        self.features.add("slots")

        prev_in_class = self._in_class
        self._in_class = True
        self._class_depth += 1
        self.generic_visit(node)
        self._class_depth -= 1
        self._in_class = prev_in_class

    # -- Async --------------------------------------------------------------

    def visit_Await(self, node: ast.Await) -> None:
        self.features.add("await_expr")
        self.generic_visit(node)

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        self.features.add("async_for")
        self.generic_visit(node)

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        self.features.add("async_with")
        self.generic_visit(node)

    # -- Comprehensions -----------------------------------------------------

    def visit_ListComp(self, node: ast.ListComp) -> None:
        self.features.add("list_comprehension")
        self._check_comprehension(node.generators)
        self.generic_visit(node)

    def visit_DictComp(self, node: ast.DictComp) -> None:
        self.features.add("dict_comprehension")
        self._check_comprehension(node.generators)
        self.generic_visit(node)

    def visit_SetComp(self, node: ast.SetComp) -> None:
        self.features.add("set_comprehension")
        self._check_comprehension(node.generators)
        self.generic_visit(node)

    def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
        self.features.add("generator_expression")
        self._check_comprehension(node.generators)
        self.generic_visit(node)

    def _check_comprehension(self, generators: list[ast.comprehension]) -> None:
        if len(generators) > 1:
            self.features.add("nested_comprehension")
        for gen in generators:
            if gen.ifs:
                self.features.add("comprehension_filter")
            if gen.is_async:
                self.features.add("async_comprehension")

    # -- Operators ----------------------------------------------------------

    def visit_BinOp(self, node: ast.BinOp) -> None:
        op_map: dict[type, str] = {
            ast.Add: "add",
            ast.Sub: "sub",
            ast.Mult: "mult",
            ast.Div: "div",
            ast.FloorDiv: "floordiv",
            ast.Mod: "mod",
            ast.Pow: "pow_op",
            ast.MatMult: "matmul",
            ast.LShift: "lshift",
            ast.RShift: "rshift",
            ast.BitOr: "bitor",
            ast.BitXor: "bitxor",
            ast.BitAnd: "bitand",
        }
        feat = op_map.get(type(node.op))
        if feat:
            self.features.add(feat)
        # Detect string concat (str + str) and repeat (str * int).
        if isinstance(node.op, ast.Add) and (
            _is_string_node(node.left) or _is_string_node(node.right)
        ):
            self.features.add("string_concat")
        if isinstance(node.op, ast.Mult) and (
            _is_string_node(node.left) or _is_string_node(node.right)
        ):
            self.features.add("string_repeat")
        self.generic_visit(node)

    def visit_UnaryOp(self, node: ast.UnaryOp) -> None:
        op_map: dict[type, str] = {
            ast.Invert: "invert",
            ast.USub: "unary_neg",
            ast.UAdd: "unary_pos",
            ast.Not: "not_op",
        }
        feat = op_map.get(type(node.op))
        if feat:
            self.features.add(feat)
        self.generic_visit(node)

    def visit_BoolOp(self, node: ast.BoolOp) -> None:
        if isinstance(node.op, ast.And):
            self.features.add("and_op")
        elif isinstance(node.op, ast.Or):
            self.features.add("or_op")
        self.generic_visit(node)

    def visit_Compare(self, node: ast.Compare) -> None:
        cmp_map: dict[type, str] = {
            ast.Eq: "eq",
            ast.NotEq: "noteq",
            ast.Lt: "lt",
            ast.LtE: "lte",
            ast.Gt: "gt",
            ast.GtE: "gte",
            ast.Is: "is_op",
            ast.IsNot: "is_not",
            ast.In: "in_op",
            ast.NotIn: "not_in",
        }
        for op in node.ops:
            feat = cmp_map.get(type(op))
            if feat:
                self.features.add(feat)
        self.generic_visit(node)

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        self.features.add("augmented_assign")
        self.generic_visit(node)

    def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
        self.features.add("walrus")
        self.generic_visit(node)

    # -- Builtin function calls ---------------------------------------------

    def visit_Call(self, node: ast.Call) -> None:
        if isinstance(node.func, ast.Name):
            name = node.func.id
            feat = _BUILTIN_FUNCTION_MAP.get(name)
            if feat:
                self.features.add(feat)
            if name == "super":
                self.features.add("super_call")
        elif isinstance(node.func, ast.Attribute):
            # Detect string/list/dict/set method calls.
            attr = node.func.attr
            if attr in {
                "join",
                "split",
                "strip",
                "lstrip",
                "rstrip",
                "lower",
                "upper",
                "replace",
                "find",
                "startswith",
                "endswith",
                "format",
                "encode",
                "decode",
                "count",
                "index",
                "capitalize",
                "title",
                "swapcase",
                "center",
                "ljust",
                "rjust",
                "zfill",
                "partition",
                "rpartition",
                "expandtabs",
                "isdigit",
                "isalpha",
                "isalnum",
                "isspace",
                "isupper",
                "islower",
                "istitle",
                "splitlines",
                "removeprefix",
                "removesuffix",
                "maketrans",
                "translate",
            }:
                self.features.add("string_method_call")
            if attr in {
                "append",
                "extend",
                "insert",
                "remove",
                "pop",
                "clear",
                "copy",
                "sort",
                "reverse",
            }:
                self.features.add("list_method_call")
            if attr in {
                "keys",
                "values",
                "items",
                "get",
                "update",
                "setdefault",
                "popitem",
                "fromkeys",
            }:
                self.features.add("dict_method_call")
            if attr in {
                "add",
                "discard",
                "union",
                "intersection",
                "difference",
                "symmetric_difference",
                "issubset",
                "issuperset",
                "isdisjoint",
            }:
                self.features.add("set_method_call")
        self.generic_visit(node)

    # -- String operations --------------------------------------------------

    def visit_JoinedStr(self, node: ast.JoinedStr) -> None:
        self.features.add("fstring")
        for value in node.values:
            if isinstance(value, ast.FormattedValue):
                self.features.add("fstring_expression")
                if value.format_spec:
                    self.features.add("fstring_format_spec")
                if value.conversion and value.conversion != -1:
                    self.features.add("fstring_conversion")
        self.generic_visit(node)

    def visit_Constant(self, node: ast.Constant) -> None:
        if isinstance(node.value, bytes):
            self.features.add("bytes_literal")
        elif isinstance(node.value, str):
            if "\n" in node.value and len(node.value) > 1:
                self.features.add("multiline_string")
        self.generic_visit(node)

    # -- Collection operations ----------------------------------------------

    def visit_List(self, node: ast.List) -> None:
        if isinstance(node.ctx, ast.Load):
            self.features.add("list_literal")
        self.generic_visit(node)

    def visit_Tuple(self, node: ast.Tuple) -> None:
        if isinstance(node.ctx, ast.Load):
            self.features.add("tuple_literal")
        self.generic_visit(node)

    def visit_Dict(self, node: ast.Dict) -> None:
        self.features.add("dict_literal")
        # Check for dict unpacking (**d in dict literal).
        for key in node.keys:
            if key is None:
                self.features.add("dict_unpack")
                break
        self.generic_visit(node)

    def visit_Set(self, node: ast.Set) -> None:
        self.features.add("set_literal")
        self.generic_visit(node)

    def visit_Subscript(self, node: ast.Subscript) -> None:
        self.features.add("subscript_access")
        if isinstance(node.slice, ast.Slice):
            self.features.add("slice_access")
            self.features.add("string_slice")
        self.generic_visit(node)

    def visit_Starred(self, node: ast.Starred) -> None:
        if isinstance(node.ctx, ast.Store):
            self.features.add("starred_unpack")
        self.generic_visit(node)

    # -- Unpacking assignment -----------------------------------------------

    def visit_Assign(self, node: ast.Assign) -> None:
        for target in node.targets:
            if isinstance(target, ast.Tuple | ast.List):
                self.features.add("unpacking_assign")
                for elt in ast.walk(target):
                    if isinstance(elt, ast.Starred):
                        self.features.add("starred_unpack")
        self.generic_visit(node)

    # -- Imports ------------------------------------------------------------

    def visit_Import(self, node: ast.Import) -> None:
        self.features.add("import_module")
        for alias in node.names:
            if alias.asname:
                self.features.add("import_alias")
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self.features.add("import_from")
        if node.level and node.level > 0:
            self.features.add("relative_import")
        for alias in node.names:
            if alias.name == "*":
                self.features.add("import_star")
            if alias.asname:
                self.features.add("import_alias")
        self.generic_visit(node)

    # -- Context managers ---------------------------------------------------

    def visit_With(self, node: ast.With) -> None:
        self.features.add("with_statement")
        for item in node.items:
            if item.optional_vars:
                self.features.add("with_as")
        if len(node.items) > 1:
            self.features.add("nested_with")
        self.generic_visit(node)

    # -- Type hints ---------------------------------------------------------

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        self.features.add("variable_annotation")
        self.generic_visit(node)

    def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
        self.features.add("type_alias")
        self.generic_visit(node)

    # -- Global / Nonlocal --------------------------------------------------

    def visit_Global(self, node: ast.Global) -> None:
        self.features.add("global_keyword")
        self.generic_visit(node)

    def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
        self.features.add("nonlocal_keyword")
        self.generic_visit(node)


# ---------------------------------------------------------------------------
# File scanner
# ---------------------------------------------------------------------------


def extract_features(filepath: Path) -> FileFeatures:
    """Parse a single Python file and extract features."""
    rel = filepath.relative_to(ROOT)
    try:
        source = filepath.read_text(encoding="utf-8")
    except OSError as exc:
        return FileFeatures(path=str(rel), error=str(exc))

    try:
        tree = ast.parse(source, filename=str(filepath))
    except SyntaxError as exc:
        return FileFeatures(path=str(rel), error=f"SyntaxError: {exc}")

    extractor = FeatureExtractor()
    extractor.visit(tree)

    # Post-processing: detect closures (nested function referencing outer vars).
    _detect_closures(tree, extractor.features)

    # Detect recursive calls.
    _detect_recursion(tree, extractor.features)

    # Detect fstring debug via source text (f"{x=}").
    if "f'" in source or 'f"' in source:
        if "=}" in source or "=!}" in source or "=:" in source:
            extractor.features.add("fstring_debug")

    # Detect raw strings via source scan.
    for line in source.splitlines():
        stripped = line.lstrip()
        if stripped.startswith(("r'", 'r"', "R'", 'R"', "rb'", 'rb"', "Rb'")):
            extractor.features.add("raw_string")
            break

    return FileFeatures(path=str(rel), features=extractor.features)


def _detect_closures(tree: ast.AST, features: set[str]) -> None:
    """Detect closure patterns: nested function accessing enclosing scope vars."""
    for node in ast.walk(tree):
        if not isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef):
            continue
        outer_names = {
            arg.arg
            for arg in node.args.args + node.args.posonlyargs + node.args.kwonlyargs
        }
        # Add local assignments.
        for child in ast.iter_child_nodes(node):
            if isinstance(child, ast.Assign):
                for target in child.targets:
                    if isinstance(target, ast.Name):
                        outer_names.add(target.id)

        for child in ast.walk(node):
            if not isinstance(child, ast.FunctionDef | ast.AsyncFunctionDef):
                continue
            if child is node:
                continue
            # Check if inner function references outer names.
            for inner_node in ast.walk(child):
                if (
                    isinstance(inner_node, ast.Name)
                    and isinstance(inner_node.ctx, ast.Load)
                    and inner_node.id in outer_names
                ):
                    features.add("closure")
                    return


def _detect_recursion(tree: ast.AST, features: set[str]) -> None:
    """Detect functions that call themselves."""
    for node in ast.walk(tree):
        if not isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef):
            continue
        fname = node.name
        for child in ast.walk(node):
            if (
                isinstance(child, ast.Call)
                and isinstance(child.func, ast.Name)
                and child.func.id == fname
            ):
                features.add("recursive_function")
                return


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------


@dataclass
class CategoryScore:
    """Coverage score for a feature category."""

    category: str
    total_features: int
    covered_features: int
    coverage_pct: float
    covered: list[str]
    gaps: list[str]


def compute_coverage(
    all_results: list[FileFeatures],
) -> tuple[
    dict[str, set[str]],  # feature -> set of file paths
    list[CategoryScore],
]:
    """Compute coverage mapping and per-category scores."""
    feature_to_files: dict[str, set[str]] = defaultdict(set)
    for result in all_results:
        for feat in result.features:
            feature_to_files[feat].add(result.path)

    scores: list[CategoryScore] = []
    for category, features in sorted(FEATURE_CATEGORIES.items()):
        covered = [f for f in features if f in feature_to_files]
        gaps = [f for f in features if f not in feature_to_files]
        total = len(features)
        pct = (len(covered) / total * 100) if total else 0.0
        scores.append(
            CategoryScore(
                category=category,
                total_features=total,
                covered_features=len(covered),
                coverage_pct=round(pct, 1),
                covered=covered,
                gaps=gaps,
            )
        )

    return dict(feature_to_files), scores


def generate_markdown_report(
    feature_to_files: dict[str, set[str]],
    scores: list[CategoryScore],
    gaps_only: bool = False,
    category_filter: str | None = None,
) -> str:
    """Generate a markdown coverage report."""
    lines: list[str] = []
    lines.append("# Differential Test Coverage Analysis")
    lines.append("")

    # Summary.
    total_features = sum(s.total_features for s in scores)
    covered_features = sum(s.covered_features for s in scores)
    overall_pct = (
        round(covered_features / total_features * 100, 1) if total_features else 0.0
    )
    lines.append("## Summary")
    lines.append("")
    lines.append(f"- **Total tracked features:** {total_features}")
    lines.append(f"- **Covered by tests:** {covered_features}")
    lines.append(f"- **Overall coverage:** {overall_pct}%")
    lines.append(
        f"- **Total test files scanned:** {len({f for files in feature_to_files.values() for f in files})}"
    )
    lines.append("")

    # Category summary table.
    lines.append("## Coverage by Category")
    lines.append("")
    lines.append("| Category | Covered | Total | Coverage |")
    lines.append("| --- | --- | --- | --- |")
    for score in scores:
        if category_filter and score.category != category_filter:
            continue
        lines.append(
            f"| {score.category} | {score.covered_features} | "
            f"{score.total_features} | {score.coverage_pct}% |"
        )
    lines.append("")

    # Per-category detail.
    for score in scores:
        if category_filter and score.category != category_filter:
            continue

        lines.append(f"## {score.category}")
        lines.append("")

        if not gaps_only:
            lines.append("### Covered Features")
            lines.append("")
            if score.covered:
                lines.append("| Feature | Test Files (sample) |")
                lines.append("| --- | --- |")
                for feat in score.covered:
                    files = sorted(feature_to_files.get(feat, set()))
                    sample = ", ".join(f"`{f}`" for f in files[:3])
                    if len(files) > 3:
                        sample += f" (+{len(files) - 3} more)"
                    lines.append(f"| {feat} | {sample} |")
            else:
                lines.append("*No features covered in this category.*")
            lines.append("")

        if score.gaps:
            lines.append("### Gaps (Untested Features)")
            lines.append("")
            for feat in score.gaps:
                lines.append(f"- `{feat}`")
            lines.append("")

    return "\n".join(lines)


def generate_json_report(
    feature_to_files: dict[str, set[str]],
    scores: list[CategoryScore],
    gaps_only: bool = False,
    category_filter: str | None = None,
) -> str:
    """Generate a JSON coverage report."""
    total_features = sum(s.total_features for s in scores)
    covered_features = sum(s.covered_features for s in scores)
    overall_pct = (
        round(covered_features / total_features * 100, 1) if total_features else 0.0
    )

    report: dict = {
        "summary": {
            "total_features": total_features,
            "covered_features": covered_features,
            "overall_coverage_pct": overall_pct,
        },
        "categories": {},
    }

    for score in scores:
        if category_filter and score.category != category_filter:
            continue
        cat_data: dict = {
            "total": score.total_features,
            "covered": score.covered_features,
            "coverage_pct": score.coverage_pct,
        }
        if not gaps_only:
            cat_data["covered_features"] = {
                feat: sorted(feature_to_files.get(feat, set()))
                for feat in score.covered
            }
        cat_data["gaps"] = score.gaps
        report["categories"][score.category] = cat_data

    return json.dumps(report, indent=2, sort_keys=True)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Analyze differential test coverage by Python feature.",
    )
    parser.add_argument(
        "--root",
        default=str(DIFF_ROOT),
        help="Root directory for differential tests.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output JSON report.",
    )
    parser.add_argument(
        "--markdown",
        action="store_true",
        help="Output markdown report (default if neither --json nor stdout).",
    )
    parser.add_argument(
        "--gaps-only",
        action="store_true",
        help="Only show coverage gaps.",
    )
    parser.add_argument(
        "--category",
        type=str,
        default=None,
        help="Filter by feature category.",
    )
    parser.add_argument(
        "--output",
        type=str,
        default=None,
        help="Write report to file instead of stdout.",
    )
    args = parser.parse_args()

    root = Path(args.root)
    if not root.is_dir():
        print(f"Error: {root} is not a directory", file=sys.stderr)
        return 1

    # Validate category filter.
    if args.category and args.category not in FEATURE_CATEGORIES:
        print(
            f"Error: unknown category '{args.category}'. "
            f"Valid categories: {', '.join(sorted(FEATURE_CATEGORIES))}",
            file=sys.stderr,
        )
        return 1

    # Collect all test files.
    test_files = sorted(root.rglob("*.py"))
    if not test_files:
        print(f"No .py files found under {root}", file=sys.stderr)
        return 1

    # Extract features from all files.
    results: list[FileFeatures] = []
    parse_errors: list[str] = []
    for filepath in test_files:
        result = extract_features(filepath)
        results.append(result)
        if result.error:
            parse_errors.append(f"  {result.path}: {result.error}")

    # Compute coverage.
    feature_to_files, scores = compute_coverage(results)

    # Generate report.
    if args.json:
        report = generate_json_report(
            feature_to_files, scores, args.gaps_only, args.category
        )
    else:
        report = generate_markdown_report(
            feature_to_files, scores, args.gaps_only, args.category
        )

    # Output.
    if args.output:
        Path(args.output).write_text(report)
        print(f"Report written to {args.output}")
    else:
        print(report)

    # Print parse errors to stderr.
    if parse_errors:
        print(f"\n{len(parse_errors)} files had parse errors:", file=sys.stderr)
        for err in parse_errors:
            print(err, file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
