"""Minimal ast support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import Any, Iterable, Iterator

__all__ = [
    "AST",
    "Add",
    "Assign",
    "BinOp",
    "Constant",
    "Expr",
    "Expression",
    "FunctionDef",
    "Load",
    "Module",
    "Name",
    "Return",
    "Store",
    "arg",
    "arguments",
    "get_docstring",
    "iter_child_nodes",
    "iter_fields",
    "parse",
    "walk",
    "PyCF_ALLOW_TOP_LEVEL_AWAIT",
]

PyCF_ALLOW_TOP_LEVEL_AWAIT = 0x1000


class AST:
    _fields: tuple[str, ...] = ()

    def __repr__(self) -> str:
        items = []
        for name in self._fields:
            items.append(name + "=" + repr(getattr(self, name)))
        return self.__class__.__name__ + "(" + ", ".join(items) + ")"


class Module(AST):
    _fields = ("body", "type_ignores")

    def __init__(self, body: list[AST] | None = None) -> None:
        self.body = list(body or [])
        self.type_ignores: list[Any] = []


class Expression(AST):
    _fields = ("body",)

    def __init__(self, body: AST) -> None:
        self.body = body


class FunctionDef(AST):
    _fields = ("name", "args", "body", "decorator_list", "returns", "type_comment")

    def __init__(self, name: str, args: "arguments", body: list[AST]) -> None:
        self.name = name
        self.args = args
        self.body = list(body)
        self.decorator_list: list[Any] = []
        self.returns: Any | None = None
        self.type_comment: Any | None = None


class arguments(AST):
    _fields = (
        "posonlyargs",
        "args",
        "vararg",
        "kwonlyargs",
        "kw_defaults",
        "kwarg",
        "defaults",
    )

    def __init__(self, args: list["arg"] | None = None) -> None:
        self.posonlyargs: list[arg] = []
        self.args = list(args or [])
        self.vararg: arg | None = None
        self.kwonlyargs: list[arg] = []
        self.kw_defaults: list[Any] = []
        self.kwarg: arg | None = None
        self.defaults: list[Any] = []


class arg(AST):
    _fields = ("arg", "annotation")

    def __init__(self, name: str) -> None:
        self.arg = name
        self.annotation: Any | None = None


class Return(AST):
    _fields = ("value",)

    def __init__(self, value: AST | None) -> None:
        self.value = value


class Expr(AST):
    _fields = ("value",)

    def __init__(self, value: AST) -> None:
        self.value = value


class Assign(AST):
    _fields = ("targets", "value", "type_comment")

    def __init__(self, targets: list[AST], value: AST) -> None:
        self.targets = list(targets)
        self.value = value
        self.type_comment: str | None = None


class Name(AST):
    _fields = ("id", "ctx")

    def __init__(self, name: str, ctx: AST) -> None:
        self.id = name
        self.ctx = ctx


class Load(AST):
    _fields: tuple[str, ...] = ()


class Store(AST):
    _fields: tuple[str, ...] = ()


class Constant(AST):
    _fields = ("value", "kind")

    def __init__(self, value: Any, kind: str | None = None) -> None:
        self.value = value
        self.kind = kind


class Add(AST):
    _fields: tuple[str, ...] = ()


class BinOp(AST):
    _fields = ("left", "op", "right")

    def __init__(self, left: AST, op: AST, right: AST) -> None:
        self.left = left
        self.op = op
        self.right = right


_MOLT_AST_PARSE = _require_intrinsic("molt_ast_parse", globals())
_MOLT_AST_WALK = _require_intrinsic("molt_ast_walk", globals())
_MOLT_AST_GET_DOCSTRING = _require_intrinsic("molt_ast_get_docstring", globals())

_AST_PARSE_CTORS = (
    Module,
    Expression,
    FunctionDef,
    arguments,
    arg,
    Return,
    Expr,
    Name,
    Load,
    Constant,
    Add,
    BinOp,
    Assign,
    Store,
)


def iter_fields(node: AST) -> Iterable[tuple[str, Any]]:
    fields = getattr(type(node), "_fields", ())
    if not isinstance(fields, (list, tuple)):
        return
    for name in fields:
        yield name, getattr(node, name)


def iter_child_nodes(node: AST) -> Iterator[AST]:
    for _field_name, value in iter_fields(node):
        if isinstance(value, AST):
            yield value
            continue
        if isinstance(value, (list, tuple)):
            for item in value:
                if isinstance(item, AST):
                    yield item


def walk(node: AST) -> Iterator[AST]:
    return iter(_MOLT_AST_WALK(node))


def get_docstring(node: AST, clean: bool = True) -> str | None:
    return _MOLT_AST_GET_DOCSTRING(node, clean)


def parse(
    source: str,
    filename: str = "<unknown>",
    mode: str = "exec",
    type_comments: bool = False,
    feature_version: Any | None = None,
) -> AST:
    return _MOLT_AST_PARSE(
        source, filename, mode, type_comments, feature_version, _AST_PARSE_CTORS
    )
