"""Invocation-scoped source AST lookup for live Python function definitions.

Runtime function identity selects the source file and definition. Each file is
read and parsed once per index; directory contents never select the surface.
The returned AST belongs to the source snapshot and must not be mutated by
consumers. Create a new index for each independent generation or inspection.
"""

from __future__ import annotations

import ast
import inspect
import tokenize
from dataclasses import dataclass
from pathlib import Path
from types import FunctionType, MethodType


class PythonSourceError(RuntimeError):
    """A live Python function could not be matched to its real source."""


@dataclass(frozen=True)
class PythonFunctionSource:
    function: FunctionType
    path: Path
    node: ast.FunctionDef | ast.AsyncFunctionDef


class PythonSourceIndex:
    def __init__(self) -> None:
        self._files: dict[
            Path, dict[tuple[int, str], ast.FunctionDef | ast.AsyncFunctionDef]
        ] = {}
        self._functions: dict[FunctionType, PythonFunctionSource] = {}

    def function(self, value: object) -> PythonFunctionSource | None:
        """Resolve a function, bound method, or static/class descriptor.

        Non-function values have no function source. Missing or ambiguous source
        for a Python function is an error, never a guessed signature/body.
        """
        if isinstance(value, (staticmethod, classmethod)):
            value = value.__func__
        if isinstance(value, MethodType):
            value = value.__func__
        if not isinstance(value, FunctionType):
            return None
        try:
            function = inspect.unwrap(value)
        except ValueError as error:
            raise PythonSourceError(f"invalid wrapper chain for {value!r}") from error
        if not isinstance(function, FunctionType):
            raise PythonSourceError(f"wrapper has no Python function source: {value!r}")
        if function in self._functions:
            return self._functions[function]

        code = function.__code__
        path = Path(code.co_filename).resolve()
        definitions = self._file_definitions(path)
        node = definitions.get((code.co_firstlineno, code.co_name))
        if node is None:
            raise PythonSourceError(
                f"no source definition for {function.__qualname__} at "
                f"{path}:{code.co_firstlineno} ({code.co_name})"
            )
        source = PythonFunctionSource(function, path, node)
        self._functions[function] = source
        return source

    def _file_definitions(
        self, path: Path
    ) -> dict[tuple[int, str], ast.FunctionDef | ast.AsyncFunctionDef]:
        if path in self._files:
            return self._files[path]
        try:
            # Respect the same source-encoding declaration Python imports use.
            with tokenize.open(path) as stream:
                tree = ast.parse(stream.read(), filename=str(path))
        except (OSError, SyntaxError, UnicodeError) as error:
            raise PythonSourceError(
                f"cannot parse Python source {path}: {error}"
            ) from error
        definitions: dict[tuple[int, str], ast.FunctionDef | ast.AsyncFunctionDef] = {}
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            # co_firstlineno includes the first decorator, not just the def.
            first_line = min(
                [node.lineno, *(item.lineno for item in node.decorator_list)]
            )
            key = (first_line, node.name)
            if key in definitions:
                raise PythonSourceError(
                    f"ambiguous function source {path}:{first_line}"
                )
            definitions[key] = node
        self._files[path] = definitions
        return definitions
