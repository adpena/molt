"""CompileWarningMixin: compile-time warning pre-scan and emission helpers.

Move-only extraction from frontend/__init__.py. These helpers own the
compile-warning state and warning emission path shared by module, expression,
function, and control-flow visitors.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Literal

from molt.frontend._types import MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class CompileWarningMixin(_MixinBase):
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
            with open(source) as f:
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
                        with open(source) as f:
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

        scope_barriers = (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
        invert_bool_msg = (
            "Bitwise inversion '~' on bool is deprecated and will be "
            "removed in Python 3.16. This returns the bitwise inversion "
            "of the underlying int object and is usually not what you "
            "expect from negating a bool. Use the 'not' operator for "
            "boolean negation or ~int(x) if you really want the bitwise "
            "inversion of the underlying int."
        )

        stack: list[tuple[ast.AST, bool, bool]] = [(module_node, False, False)]
        while stack:
            node, in_finally, finally_checks_blocked = stack.pop()

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

            if in_finally and not finally_checks_blocked:
                warn_msg = None
                if isinstance(node, ast.Return):
                    warn_msg = "'return' in a 'finally' block"
                elif isinstance(node, ast.Break):
                    warn_msg = "'break' in a 'finally' block"
                elif isinstance(node, ast.Continue):
                    warn_msg = "'continue' in a 'finally' block"
                if warn_msg is not None:
                    record_warning(
                        getattr(node, "lineno", 0),
                        "SyntaxWarning",
                        warn_msg,
                    )

            child_finally_checks_blocked = finally_checks_blocked or isinstance(
                node, scope_barriers
            )
            child_entries: list[tuple[ast.AST, bool, bool]] = []
            if isinstance(node, ast.Try):
                for field_name, value in ast.iter_fields(node):
                    if isinstance(value, list):
                        children = [item for item in value if isinstance(item, ast.AST)]
                    elif isinstance(value, ast.AST):
                        children = [value]
                    else:
                        continue
                    child_in_finally = in_finally or field_name == "finalbody"
                    for child in children:
                        child_entries.append(
                            (
                                child,
                                child_in_finally,
                                child_finally_checks_blocked,
                            )
                        )
            else:
                for child in ast.iter_child_nodes(node):
                    child_entries.append(
                        (
                            child,
                            in_finally,
                            child_finally_checks_blocked,
                        )
                    )
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

        Deduplicated: each (file, line, message) triple is emitted at most
        once per process, matching CPython's behaviour.
        """
        import warnings

        lineno = getattr(node, "lineno", 0)
        source = self.source_path or "<string>"
        key = (source, lineno, message)
        if key in self._emitted_syntax_warnings:
            return
        self._emitted_syntax_warnings.add(key)
        warnings.warn_explicit(
            message,
            SyntaxWarning,
            source,
            lineno,
        )
