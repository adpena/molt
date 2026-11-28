"""Source-level proofs for exact and bounded string spellings."""

import ast
from dataclasses import dataclass
from typing import TypeAlias


# Sorted by the formal's lexical slot, including keyword-only parameters.
# Omitted slots carry no value assertion (in particular, a bound receiver).
SourceStringArguments: TypeAlias = tuple[tuple[int, str], ...]


def encode_source_string_arguments(
    arguments: SourceStringArguments,
) -> list[list[int | str]]:
    return [[slot, value] for slot, value in arguments]


def decode_source_string_arguments(raw: object) -> SourceStringArguments:
    if not isinstance(raw, list):
        raise ValueError("source string arguments must be an array")
    arguments = []
    previous = -1
    for row in raw:
        if (
            not isinstance(row, list)
            or len(row) != 2
            or type(row[0]) is not int
            or row[0] <= previous
            or type(row[1]) is not str
        ):
            raise ValueError("source string arguments must have distinct ordered slots")
        slot, value = row
        arguments.append((slot, value))
        previous = slot
    return tuple(arguments)


@dataclass(frozen=True, slots=True, order=True)
class PythonStringBound:
    """A computed string's mandatory prefix, suffix, and minimum length."""

    prefix: str
    suffix: str
    minimum: int

    def __post_init__(self) -> None:
        if self.minimum < max(len(self.prefix), len(self.suffix)) or not (
            self.prefix or self.suffix
        ):
            raise ValueError("invalid string bound")

    def admits(self, name: str) -> bool:
        return (
            len(name) >= self.minimum
            and name.startswith(self.prefix)
            and name.endswith(self.suffix)
        )

    def payload(self) -> list[object]:
        return [self.prefix, self.suffix, self.minimum]


def decode_python_string_bound(raw: object) -> PythonStringBound:
    if (
        not isinstance(raw, list)
        or len(raw) != 3
        or type(raw[0]) is not str
        or type(raw[1]) is not str
        or type(raw[2]) is not int
    ):
        raise ValueError("invalid string bound")
    return PythonStringBound(raw[0], raw[1], raw[2])


def _string_parts(node: ast.expr) -> list[str | None] | None:
    """The literal and computed parts of a string expression, in order, or
    None when it is not a concatenation of string parts."""
    parts: list[str | None] = []
    pending = [node]
    while pending:
        expression = pending.pop()
        if isinstance(expression, ast.Constant) and type(expression.value) is str:
            parts.append(expression.value)
        elif isinstance(expression, ast.BinOp) and isinstance(expression.op, ast.Add):
            pending.extend((expression.right, expression.left))
        elif isinstance(expression, ast.JoinedStr):
            for part in expression.values:
                if isinstance(part, ast.Constant) and type(part.value) is str:
                    parts.append(part.value)
                elif isinstance(part, ast.FormattedValue) and expression is node:
                    parts.append(None)
                else:
                    # Addition may dispatch to either operand's Python method.
                    # Even a lone f-string field can return a str subclass.
                    return None
        else:
            return None
    return parts


def python_string_spelling(
    node: ast.expr | str | None,
) -> str | PythonStringBound | None:
    """An exact string spelling or a bound on every successful result.

    None makes no claim. The expression may still evaluate user code or fail.
    """
    if isinstance(node, str):
        return node if type(node) is str else None
    if isinstance(node, ast.Constant):
        return node.value if type(node.value) is str else None
    if node is None:
        return None
    parts = _string_parts(node)
    if parts is None:
        return None
    if all(part is not None for part in parts):
        return "".join(part for part in parts if part is not None)
    leading = []
    for part in parts:
        if part is None:
            break
        leading.append(part)
    trailing = []
    for part in reversed(parts):
        if part is None:
            break
        trailing.append(part)
    prefix = "".join(leading)
    suffix = "".join(reversed(trailing))
    if not (prefix or suffix):
        return None
    minimum = sum(len(part) for part in parts if part is not None)
    return PythonStringBound(prefix, suffix, minimum)
