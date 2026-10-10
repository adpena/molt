"""CompileWarningMixin: compile-time warning pre-scan and emission helpers.

Move-only extraction from frontend/__init__.py. These helpers own the
compile-warning state and warning emission path shared by module, expression,
function, and control-flow visitors.
"""

from __future__ import annotations

import ast
from typing import Literal

from molt.frontend._types import MoltOp, MoltValue
from molt.frontend._mixin_base import GeneratorMixinBase


class CompileWarningMixin(GeneratorMixinBase):
    _emitted_syntax_warnings: set[tuple[str, int, str]]
    _deferred_runtime_warnings: list[str]

    def _emit_deprecation_warning(self, node: ast.AST, message: str) -> None:
        """Emit a DeprecationWarning to stderr, matching CPython's format."""
        lineno = getattr(node, "lineno", 0)
        source = self.source_path or "<string>"
        key = (source, lineno, message)
        if key in self._emitted_syntax_warnings:
            return
        self._emitted_syntax_warnings.add(key)
        # Read the source line for context (matches CPython's warning format).
        src_line = ""
        try:
            with open(source, encoding="utf-8") as f:
                for i, line in enumerate(f, 1):
                    if i == lineno:
                        src_line = line.rstrip()
                        break
        except (OSError, UnicodeDecodeError):
            pass
        import sys

        print(f"{source}:{lineno}: DeprecationWarning: {message}", file=sys.stderr)
        if src_line:
            print(f"  {src_line}", file=sys.stderr)

    def _prescan_compile_warnings(self, module_node: ast.Module) -> None:
        """Pre-scan AST for patterns that need compile-time warnings."""
        source = self.source_path or "<string>"
        cached_source_lines: list[str] | None | Literal[False] = False

        def source_line_for(lineno: int) -> str | None:
            nonlocal cached_source_lines
            if cached_source_lines is False:
                if source == "<string>":
                    cached_source_lines = None
                else:
                    try:
                        with open(source, encoding="utf-8") as f:
                            cached_source_lines = [line.rstrip("\n") for line in f]
                    except (OSError, UnicodeDecodeError):
                        cached_source_lines = None
            if (
                not cached_source_lines
                or lineno <= 0
                or lineno > len(cached_source_lines)
            ):
                return None
            return cached_source_lines[lineno - 1].strip()

        def record_warning(lineno: int, category: str, message: str) -> None:
            key = (source, lineno, message)
            if key in self._emitted_syntax_warnings:
                return
            self._emitted_syntax_warnings.add(key)
            self._deferred_runtime_warnings.append(
                f"{source}:{lineno}: {category}: {message}"
            )
            src_line = source_line_for(lineno)
            if src_line:
                self._deferred_runtime_warnings.append(f"  {src_line}")

        invert_bool_msg = (
            "Bitwise inversion '~' on bool is deprecated and will be "
            "removed in Python 3.16. This returns the bitwise inversion "
            "of the underlying int object and is usually not what you "
            "expect from negating a bool. Use the 'not' operator for "
            "boolean negation or ~int(x) if you really want the bitwise "
            "inversion of the underlying int."
        )

        stack: list[ast.AST] = [module_node]
        while stack:
            node = stack.pop()
            if (
                isinstance(node, ast.UnaryOp)
                and isinstance(node.op, ast.Invert)
                and isinstance(node.operand, ast.Constant)
                and isinstance(node.operand.value, bool)
            ):
                record_warning(
                    getattr(node, "lineno", 0),
                    "DeprecationWarning",
                    invert_bool_msg,
                )

            stack.extend(reversed(list(ast.iter_child_nodes(node))))

    def _emit_finally_transfer_warnings(self, module_node: ast.Module) -> None:
        """Emit CPython 3.14 PEP765 diagnostics once, before source pruning."""
        if self.target_python < (3, 14):
            return
        # CPython ast_preprocess uses the innermost function, loop-body or
        # finally context. Loop else retains its enclosing context. A function's
        # own finally replaces its function context; classes introduce none.
        stack: list[tuple[ast.AST, tuple[bool, bool, bool]]] = [
            (module_node, (False, False, False))
        ]
        while stack:
            node, context = stack.pop()
            in_finally, in_function, in_loop = context
            if in_finally:
                warn_msg = None
                if isinstance(node, ast.Return) and not in_function:
                    warn_msg = "'return' in a 'finally' block"
                elif isinstance(node, ast.Break) and not in_loop:
                    warn_msg = "'break' in a 'finally' block"
                elif isinstance(node, ast.Continue) and not in_loop:
                    warn_msg = "'continue' in a 'finally' block"
                if warn_msg is not None:
                    self._emit_syntax_warning(node, warn_msg)

            child_entries: list[tuple[ast.AST, tuple[bool, bool, bool]]] = []
            for field_name, value in ast.iter_fields(node):
                child_context = context
                if field_name == "body":
                    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                        child_context = (False, True, False)
                    elif isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
                        child_context = (False, False, True)
                elif field_name == "finalbody" and isinstance(
                    node, (ast.Try, ast.TryStar)
                ):
                    child_context = (True, False, False)
                if isinstance(value, list):
                    child_entries.extend(
                        (child, child_context)
                        for child in value
                        if isinstance(child, ast.AST)
                    )
                elif isinstance(value, ast.AST):
                    child_entries.append((value, child_context))
            stack.extend(reversed(child_entries))

    def _emit_deferred_warnings(self) -> None:
        """Emit deferred runtime warnings as WARN_STDERR ops.

        Called at the start of module compilation so warnings appear before
        any print output, matching CPython's behavior of emitting compile-time
        warnings before executing any code.
        """
        for line in self._deferred_runtime_warnings:
            val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[line], result=val))
            self.emit(MoltOp(kind="WARN_STDERR", args=[val], result=MoltValue("none")))
        self._deferred_runtime_warnings.clear()

    def _emit_syntax_warning(self, node: ast.AST, message: str) -> None:
        """Emit a SyntaxWarning to stderr, matching CPython's format.

        The single source pass owns emission, including multiple transfers on
        one line. A warnings-as-errors filter becomes SyntaxError, as in
        CPython's compiler diagnostic authority.
        """
        import warnings

        lineno = getattr(node, "lineno", 0)
        source = self.source_path or "<string>"
        try:
            warnings.warn_explicit(message, SyntaxWarning, source, lineno)
        except SyntaxWarning:
            import tokenize

            # CPython reads the current encoded source file for SyntaxError;
            # linecache may contain stale or entirely virtual source text.
            text = None
            try:
                with tokenize.open(source) as stream:
                    text = next(
                        (
                            line
                            for index, line in enumerate(stream, 1)
                            if index == lineno
                        ),
                        None,
                    )
            except (OSError, UnicodeError, LookupError, SyntaxError):
                pass
            raise SyntaxError(
                message,
                (
                    source,
                    lineno,
                    getattr(node, "col_offset", 0) + 1,
                    text,
                    getattr(node, "end_lineno", lineno),
                    getattr(node, "end_col_offset", 0) + 1,
                ),
            ) from None
