"""Shared AST syntax admission and source-only version-delta metadata.

CPython's feature_version is best effort. Neither a parsed AST nor this
minimum-version gate attests implementation or target execution support.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from enum import Enum
from typing import Literal


class PythonSyntaxFamily(str, Enum):
    COMPREHENSION_UNPACKING = "comprehension-unpacking"
    LAZY_IMPORTS = "lazy-imports"


@dataclass(frozen=True)
class PythonSyntaxDelta:
    family: PythonSyntaxFamily
    minimum_version: tuple[int, int]
    pep_url: str
    disposition: Literal["minimum-version-gated", "unsupported"]
    witness_paths: tuple[str, ...]


_COMPREHENSION_UNPACKING = PythonSyntaxDelta(
    PythonSyntaxFamily.COMPREHENSION_UNPACKING,
    (3, 15),
    "https://peps.python.org/pep-0798/",
    "minimum-version-gated",
    ("tests/cli/test_cli_target_python.py", "tests/test_python_binding_flow.py"),
)
_LAZY_IMPORTS = PythonSyntaxDelta(
    PythonSyntaxFamily.LAZY_IMPORTS,
    (3, 15),
    "https://peps.python.org/pep-0810/",
    "unsupported",
    ("tests/cli/test_cli_target_python.py", "tests/test_python_binding_flow.py"),
)
PYTHON_SYNTAX_DELTAS = (_COMPREHENSION_UNPACKING, _LAZY_IMPORTS)
PYTHON_SYNTAX_NODE_TYPES = (
    ast.ListComp,
    ast.SetComp,
    ast.DictComp,
    ast.GeneratorExp,
    ast.Import,
    ast.ImportFrom,
)


def require_python_syntax_target(
    node: ast.ListComp
    | ast.SetComp
    | ast.DictComp
    | ast.GeneratorExp
    | ast.Import
    | ast.ImportFrom,
    target_python: tuple[int, int],
    *,
    filename: str | None = None,
) -> None:
    if isinstance(node, (ast.Import, ast.ImportFrom)):
        # Pinned v3.15.0 ASDL adds optional integer is_lazy to both statements.
        # Refuse before analysis can silently manufacture eager import facts.
        lazy = getattr(node, "is_lazy", None)
        if lazy is not None and (type(lazy) not in (bool, int) or lazy not in (0, 1)):
            raise SyntaxError("invalid import is_lazy metadata")
        if not lazy:
            return
        minimum = ".".join(map(str, _LAZY_IMPORTS.minimum_version))
        message = (
            f"lazy imports require Python {minimum} or newer (PEP 810)"
            if target_python < _LAZY_IMPORTS.minimum_version
            else "lazy imports (PEP 810) are not implemented; eager execution is refused"
        )
        raise SyntaxError(
            message,
            (
                filename,
                getattr(node, "lineno", 1),
                getattr(node, "col_offset", 0) + 1,
                None,
            ),
        )
    expanded = (
        node.value is None
        if isinstance(node, ast.DictComp)
        else isinstance(node.elt, ast.Starred)
    )
    if target_python < _COMPREHENSION_UNPACKING.minimum_version and expanded:
        message = (
            "dict unpacking cannot be used in dict comprehension"
            if isinstance(node, ast.DictComp)
            else "iterable unpacking cannot be used in comprehension"
        )
        raise SyntaxError(
            message,
            (
                filename,
                getattr(node, "lineno", 1),
                getattr(node, "col_offset", 0) + 1,
                None,
            ),
        )
