"""Development-side parser for the canonical intrinsic declarations.

Declaration identity and public Python metadata are distinct from the executable
symbol. Runtime registries and WASM imports project the same validated facts.

An optional ``# cfg: ...`` line declares the Rust ``cfg`` under which the
entry's executable is defined: at most one Cargo feature plus target
restrictions, spelled exactly as the generated ``#[cfg(...)]`` spells them.
The declaration is the only availability fact for the symbol (HF-487). The
generated resolvers prove at compile time that the definition's ``cfg`` equals
it, and reachability selects or refuses the feature from it.
"""

from __future__ import annotations

from dataclasses import dataclass
import re

_DEF_RE = re.compile(
    r"^def\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\((?P<params>.*)\)\s*->\s*(?P<ret>[^:]+):\s*\.\.\.\s*$"
)
_IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
_FEATURE_RE = re.compile(r'feature = "(?P<value>[A-Za-z0-9_-]+)"')
_NOT_ARCH_RE = re.compile(rf'not\(target_arch = "(?P<value>{_IDENT})"\)')
_OS_RE = re.compile(rf'target_os = "(?P<value>{_IDENT})"')
_ANY_OS_RE = re.compile(r"any\((?P<body>[^()]*)\)")
_NOT_OS_RE = re.compile(rf'not\(target_os = "(?P<value>{_IDENT})"\)')


@dataclass(frozen=True, slots=True)
class RuntimeCfg:
    """The Rust ``cfg`` under which one runtime executable is defined.

    ``feature`` is the Cargo feature that links the definition; the target
    fields restrict it further. An empty value means "always defined".
    """

    feature: str | None = None
    unsupported_target_arches: tuple[str, ...] = ()
    supported_target_oses: tuple[str, ...] = ()
    unsupported_target_oses: tuple[str, ...] = ()

    def conjuncts(self) -> tuple[str, ...]:
        """The canonical ``cfg`` conjuncts, in declaration order."""
        parts: list[str] = []
        if self.feature is not None:
            parts.append(f'feature = "{self.feature}"')
        parts.extend(
            f'not(target_arch = "{arch}")' for arch in self.unsupported_target_arches
        )
        if len(self.supported_target_oses) == 1:
            parts.append(f'target_os = "{self.supported_target_oses[0]}"')
        elif self.supported_target_oses:
            parts.append(
                "any("
                + ", ".join(f'target_os = "{os}"' for os in self.supported_target_oses)
                + ")"
            )
        parts.extend(f'not(target_os = "{os}")' for os in self.unsupported_target_oses)
        return tuple(parts)

    def declaration(self) -> str | None:
        """The manifest spelling (``# cfg: <declaration>``), or None."""
        parts = self.conjuncts()
        return ", ".join(parts) if parts else None

    def rust_predicate(self) -> str | None:
        """The ``#[cfg(...)]`` predicate, or None for an always-defined symbol."""
        parts = self.conjuncts()
        if not parts:
            return None
        if len(parts) == 1:
            return parts[0]
        return f"all({', '.join(parts)})"

    def available_on_target(
        self, *, target_arch: str | None, target_os: str | None
    ) -> bool:
        """Whether some feature set defines the symbol on this target."""
        if target_arch is not None and target_arch in self.unsupported_target_arches:
            return False
        if self.supported_target_oses and target_os not in self.supported_target_oses:
            return False
        return target_os is None or target_os not in self.unsupported_target_oses


ALWAYS_DEFINED = RuntimeCfg()


def _split_top_level(text: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    current: list[str] = []
    for ch in text:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth < 0:
                raise ValueError(f"unbalanced cfg declaration: {text!r}")
        if ch == "," and depth == 0:
            parts.append("".join(current).strip())
            current = []
            continue
        current.append(ch)
    if depth != 0:
        raise ValueError(f"unbalanced cfg declaration: {text!r}")
    parts.append("".join(current).strip())
    return parts


def parse_runtime_cfg(text: str) -> RuntimeCfg:
    """Parse one ``# cfg:`` declaration; reject non-canonical spellings."""
    feature: str | None = None
    arches: list[str] = []
    oses: list[str] = []
    excluded_oses: list[str] = []
    for part in _split_top_level(text):
        if match := _FEATURE_RE.fullmatch(part):
            if feature is not None:
                raise ValueError(f"cfg declares more than one feature: {text!r}")
            feature = match["value"]
        elif match := _NOT_ARCH_RE.fullmatch(part):
            arches.append(match["value"])
        elif match := _OS_RE.fullmatch(part):
            if oses:
                raise ValueError(f"cfg declares target_os twice: {text!r}")
            oses.append(match["value"])
        elif match := _ANY_OS_RE.fullmatch(part):
            if oses:
                raise ValueError(f"cfg declares target_os twice: {text!r}")
            members = _split_top_level(match["body"])
            for member in members:
                os_match = _OS_RE.fullmatch(member)
                if os_match is None:
                    raise ValueError(f"any(...) admits only target_os: {text!r}")
                oses.append(os_match["value"])
            if len(oses) < 2:
                raise ValueError(f"any(...) needs two or more target_os: {text!r}")
        elif match := _NOT_OS_RE.fullmatch(part):
            excluded_oses.append(match["value"])
        else:
            raise ValueError(f"unsupported cfg conjunct {part!r} in {text!r}")
    if oses and excluded_oses:
        raise ValueError(f"cfg both requires and excludes target_os: {text!r}")
    for values, label in (
        (arches, "target_arch"),
        (oses, "target_os"),
        (excluded_oses, "target_os"),
    ):
        if len(set(values)) != len(values):
            raise ValueError(f"cfg repeats a {label}: {text!r}")
    cfg = RuntimeCfg(
        feature=feature,
        unsupported_target_arches=tuple(arches),
        supported_target_oses=tuple(oses),
        unsupported_target_oses=tuple(excluded_oses),
    )
    if cfg.declaration() != text:
        raise ValueError(
            f"cfg declaration {text!r} is not canonical; write {cfg.declaration()!r}"
        )
    return cfg


class IntrinsicEntry:
    __slots__ = (
        "name",
        "symbol",
        "arity",
        "defaults",
        "python_name",
        "cfg",
        "parameter_names",
    )

    def __init__(
        self,
        name: str,
        symbol: str,
        arity: int,
        defaults: tuple[str, ...] = (),
        python_name: str | None = None,
        cfg: RuntimeCfg = ALWAYS_DEFINED,
        *,
        parameter_names: tuple[str, ...],
    ) -> None:
        if (
            len(parameter_names) != arity
            or any(not name.isidentifier() for name in parameter_names)
            or len(set(parameter_names)) != len(parameter_names)
        ):
            raise ValueError("intrinsic parameter names must match its fixed arity")
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
        self.cfg = cfg
        self.parameter_names = parameter_names


def _iter_defs(text: str) -> list[tuple[str, dict[str, str]]]:
    defs: list[tuple[str, dict[str, str]]] = []
    buf: list[str] = []
    paren_depth = 0
    metadata: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not buf:
            if line.startswith("# cfg"):
                match = re.fullmatch(r"# cfg: (\S.*)", line)
                if match is None:
                    raise ValueError("invalid intrinsic cfg declaration")
                if "cfg" in metadata:
                    raise ValueError("duplicate cfg declaration")
                metadata["cfg"] = match.group(1)
                continue
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
        if "cfg" in metadata and "runtime-symbol" in metadata:
            raise ValueError(
                f"{name} shares an executable; its cfg belongs to the canonical "
                "declaration"
            )
        try:
            cfg = parse_runtime_cfg(metadata["cfg"]) if "cfg" in metadata else None
        except ValueError as exc:
            raise ValueError(f"{name}: {exc}") from exc
        if cfg == ALWAYS_DEFINED:
            raise ValueError(f"{name}: an empty cfg declaration is not allowed")
        entries[name] = IntrinsicEntry(
            name=name,
            symbol=metadata.get("runtime-symbol", name),
            arity=len(params),
            defaults=_parse_intrinsic_defaults(name, params),
            python_name=metadata.get("python-name"),
            cfg=cfg or ALWAYS_DEFINED,
            parameter_names=tuple(
                param.split(":", 1)[0].split("=", 1)[0].strip() for param in params
            ),
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
        entry.cfg = target.cfg
    return list(entries.values())
