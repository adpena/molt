"""Minimal ast support for Molt."""

from __future__ import annotations

from typing import Any, Iterable, Iterator

__all__ = [
    "AST",
    "Add",
    "BinOp",
    "Constant",
    "Expr",
    "Expression",
    "FunctionDef",
    "Load",
    "Module",
    "Name",
    "Return",
    "arg",
    "arguments",
    "get_docstring",
    "iter_fields",
    "parse",
    "walk",
    "PyCF_ALLOW_TOP_LEVEL_AWAIT",
]

PyCF_ALLOW_TOP_LEVEL_AWAIT = 0x1000

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): wire
# ast.parse to the Rust ruff-ast frontend and expose full AST node parity.


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


class Name(AST):
    _fields = ("id", "ctx")

    def __init__(self, name: str, ctx: AST) -> None:
        self.id = name
        self.ctx = ctx


class Load(AST):
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


def iter_fields(node: AST) -> Iterable[tuple[str, Any]]:
    for name in getattr(node, "_fields", ()):
        yield name, getattr(node, name)


def walk(node: AST) -> Iterator[AST]:
    stack: list[AST] = [node]
    while stack:
        current = stack.pop()
        yield current
        for child in _iter_child_nodes(current):
            stack.append(child)


def get_docstring(node: AST, clean: bool = True) -> str | None:
    del clean
    body = getattr(node, "body", None)
    if not isinstance(body, list) or not body:
        return None
    first = body[0]
    if isinstance(first, Expr) and isinstance(first.value, Constant):
        if isinstance(first.value.value, str):
            return first.value.value
    return None


def _iter_child_nodes(node: AST) -> list[AST]:
    node_type = type(node)
    if node_type is Module:
        return list(node.body)
    if node_type is Expression:
        return [node.body]
    if node_type is FunctionDef:
        children: list[AST] = [node.args]
        children.extend(node.body)
        return children
    if node_type is arguments:
        return list(node.args)
    if node_type is Return:
        return [node.value] if node.value is not None else []
    if node_type is Expr:
        return [node.value]
    if node_type is BinOp:
        return [node.left, node.op, node.right]
    if node_type is Name:
        return [node.ctx]
    return []


def _parse_string_literal(text: str) -> str:
    text = text.strip()
    if text.startswith(('"""', "'''")) and text.endswith(('"""', "'''")):
        return text[3:-3]
    if text.startswith(('"', "'")) and text.endswith(('"', "'")):
        return text[1:-1]
    return text


def _parse_expr(text: str) -> AST:
    text = text.strip()
    if "+" in text:
        left_text, right_text = text.split("+", 1)
        return BinOp(_parse_expr(left_text), Add(), _parse_expr(right_text))
    if _is_decimal(text):
        return Constant(int(text))
    if text.startswith(('"', "'")):
        return Constant(_parse_string_literal(text))
    return Name(text, Load())


def _is_decimal(text: str) -> bool:
    if not text:
        return False
    for ch in text:
        if ch < "0" or ch > "9":
            return False
    return True


def _parse_function(source: str) -> FunctionDef:
    lines = source.splitlines()
    header = lines[0].strip()
    if not header.startswith("def "):
        raise SyntaxError("invalid function syntax")
    open_paren = header.find("(")
    close_paren = header.rfind(")")
    if open_paren == -1 or close_paren == -1 or close_paren < open_paren:
        raise SyntaxError("invalid function syntax")
    name = header[4:open_paren].strip()
    args_text = header[open_paren + 1 : close_paren]
    args_list: list[arg] = []
    for item in [part.strip() for part in args_text.split(",") if part.strip()]:
        args_list.append(arg(item))
    body_nodes: list[AST] = []
    for raw in lines[1:]:
        line = raw.strip()
        if not line:
            continue
        if not body_nodes:
            if line.startswith(('"""', "'''", '"', "'")) and line.endswith(
                ('"""', "'''", '"', "'")
            ):
                doc = _parse_string_literal(line)
                body_nodes.append(Expr(Constant(doc)))
                continue
        if line.startswith("return "):
            expr_text = line[len("return ") :].strip()
            body_nodes.append(Return(_parse_expr(expr_text)))
            continue
    return FunctionDef(name, arguments(args_list), body_nodes)


def parse(
    source: str,
    filename: str = "<unknown>",
    mode: str = "exec",
    type_comments: bool = False,
    feature_version: Any | None = None,
) -> AST:
    del filename, type_comments, feature_version
    text = source.strip()
    if mode == "eval":
        return Expression(_parse_expr(text))
    if mode != "exec":
        raise ValueError("mode must be 'exec' or 'eval'")
    if text.startswith("def "):
        return Module([_parse_function(text)])
    if text:
        return Module([Expr(_parse_expr(text))])
    return Module([])
