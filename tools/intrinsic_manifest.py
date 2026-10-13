"""Development-side parser for the canonical intrinsic declarations.

Declaration identity and public Python metadata are distinct from the executable
symbol. Runtime registries and WASM imports project the same validated facts.
"""

from __future__ import annotations

import re

_DEF_RE = re.compile(
    r"^def\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\((?P<params>.*)\)\s*->\s*(?P<ret>[^:]+):\s*\.\.\.\s*$"
)


class IntrinsicEntry:
    __slots__ = ("name", "symbol", "arity", "defaults", "python_name")

    def __init__(
        self,
        name: str,
        symbol: str,
        arity: int,
        defaults: tuple[str, ...] = (),
        python_name: str | None = None,
    ) -> None:
        if python_name is not None and (
            not isinstance(python_name, str)
            or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", python_name) is None
        ):
            raise ValueError("python-name must be an ASCII identifier")
        self.name = name
        self.symbol = symbol
        self.arity = arity
        self.defaults = defaults
        self.python_name = python_name


def _iter_defs(text: str) -> list[tuple[str, dict[str, str]]]:
    defs: list[tuple[str, dict[str, str]]] = []
    buf: list[str] = []
    paren_depth = 0
    metadata: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not buf:
            if line.startswith(("# python-name", "# runtime-symbol")):
                match = re.fullmatch(
                    r"# (python-name|runtime-symbol): ([A-Za-z_][A-Za-z0-9_]*)", line
                )
                if match is None:
                    raise ValueError("invalid intrinsic metadata declaration")
                key, value = match.groups()
                if key in metadata:
                    raise ValueError(f"duplicate {key} declaration")
                metadata[key] = value
                continue
            if not line.startswith("def "):
                if metadata and line and not line.startswith("#"):
                    raise ValueError("intrinsic metadata must precede a declaration")
                continue
        if not line:
            continue
        buf.append(line)
        paren_depth += line.count("(") - line.count(")")
        if paren_depth <= 0 and line.endswith("..."):
            defs.append((" ".join(buf), metadata))
            buf = []
            metadata = {}
    if buf:
        raise RuntimeError("unterminated def block in manifest")
    if metadata:
        raise ValueError("orphan intrinsic metadata declaration")
    return defs


def _split_params(raw: str) -> list[str]:
    params = raw.strip()
    if not params:
        return []
    out: list[str] = []
    cur: list[str] = []
    depth = 0
    for ch in params:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            part = "".join(cur).strip()
            out.append(part)
            cur = []
            continue
        cur.append(ch)
    tail = "".join(cur).strip()
    if tail:
        out.append(tail)
    return [p for p in out if p and p != "*"]


def _param_default_expr(param: str) -> str | None:
    depth = 0
    for i, ch in enumerate(param):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == "=" and depth == 0:
            return param[i + 1 :].strip()
    return None


def _rust_intrinsic_default(default_expr: str, intrinsic_name: str) -> str | None:
    if default_expr == "...":
        return None
    if default_expr == "None":
        return "IntrinsicDefaultValue::None"
    if default_expr == "True":
        return "IntrinsicDefaultValue::Bool(true)"
    if default_expr == "False":
        return "IntrinsicDefaultValue::Bool(false)"
    if re.fullmatch(r"-?\d+", default_expr):
        return f"IntrinsicDefaultValue::Int({int(default_expr)})"
    raise RuntimeError(
        f"unsupported concrete default for {intrinsic_name}: {default_expr!r}"
    )


def _parse_intrinsic_defaults(name: str, params: list[str]) -> tuple[str, ...]:
    parsed: list[str | None] = []
    for param in params:
        default_expr = _param_default_expr(param)
        parsed.append(
            None
            if default_expr is None
            else _rust_intrinsic_default(default_expr, name)
        )
    concrete_positions = [idx for idx, value in enumerate(parsed) if value is not None]
    if not concrete_positions:
        return ()
    first = concrete_positions[0]
    expected = list(range(first, len(parsed)))
    if concrete_positions != expected:
        raise RuntimeError(
            f"concrete defaults for {name} must form a trailing positional suffix"
        )
    return tuple(value for value in parsed[first:] if value is not None)


def parse_intrinsic_manifest(text: str) -> list[IntrinsicEntry]:
    entries: dict[str, IntrinsicEntry] = {}
    signatures: dict[str, tuple[tuple[str, ...], str]] = {}
    for declaration, metadata in _iter_defs(text):
        match = _DEF_RE.fullmatch(declaration)
        if match is None:
            raise RuntimeError(f"failed to parse def: {declaration}")
        name = match["name"]
        if name in entries:
            raise ValueError(f"duplicate intrinsic name: {name}")
        params = _split_params(match["params"])
        entries[name] = IntrinsicEntry(
            name=name,
            symbol=metadata.get("runtime-symbol", name),
            arity=len(params),
            defaults=_parse_intrinsic_defaults(name, params),
            python_name=metadata.get("python-name"),
        )
        signatures[name] = (tuple(params), match["ret"].strip())
    for entry in entries.values():
        if entry.symbol == entry.name:
            continue
        target = entries.get(entry.symbol)
        if target is None or target.symbol != target.name:
            raise ValueError(
                f"runtime-symbol for {entry.name} must name a canonical declaration"
            )
        if signatures[entry.name] != signatures[target.name]:
            raise ValueError(f"runtime-symbol signature mismatch for {entry.name}")
    return list(entries.values())
