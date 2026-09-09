"""Cargo target predicates and flag selection over rustc-reported cfg facts.

Precedence and the two-pass convergence boundary follow Cargo 0.97's
core/compiler/build_context/target_info.rs. Nonconvergent selection is rejected
instead of publishing a runtime identity for Cargo's warning-only ambiguity.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Literal


CfgFacts = frozenset[tuple[str, str | None]]
CARGO_QUERY_CRATE_TYPES = ("bin", "rlib", "dylib", "cdylib", "staticlib", "proc-macro")
CARGO_QUERY_CRATE_NAME = "___"
_TOKEN = re.compile(
    r'\s*(?:(?P<name>[A-Za-z_][A-Za-z_0-9]*)|(?P<string>"(?:[^"\\]|\\.)*")|(?P<punct>[(),=]))'
)


@dataclass(frozen=True, slots=True)
class CargoCfgPredicate:
    kind: Literal["atom", "all", "any", "not"]
    name: str = ""
    value: str | None = None
    children: tuple[CargoCfgPredicate, ...] = ()

    def matches(self, facts: CfgFacts) -> bool:
        if self.kind == "atom":
            return (self.name, self.value) in facts
        if self.kind == "not":
            return not self.children[0].matches(facts)
        if self.kind == "all":
            return all(child.matches(facts) for child in self.children)
        return any(child.matches(facts) for child in self.children)


def parse_cargo_cfg(text: str) -> CargoCfgPredicate:
    tokens: list[str] = []
    offset = 0
    while offset < len(text.rstrip()):
        match = _TOKEN.match(text, offset)
        if match is None:
            raise ValueError(
                f"invalid Cargo cfg predicate at offset {offset}: {text!r}"
            )
        tokens.append(match.group().strip())
        offset = match.end()
    position = 0

    def take(expected: str | None = None) -> str:
        nonlocal position
        if (
            position >= len(tokens)
            or expected is not None
            and tokens[position] != expected
        ):
            raise ValueError(
                f"invalid Cargo cfg predicate: expected {expected!r} in {text!r}"
            )
        token = tokens[position]
        position += 1
        return token

    def expression(depth: int = 0) -> CargoCfgPredicate:
        if depth > 64:
            raise ValueError("Cargo cfg predicate nesting exceeds 64")
        name = take()
        if re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", name) is None:
            raise ValueError(f"invalid Cargo cfg name: {name!r}")
        next_token = tokens[position] if position < len(tokens) else None
        if next_token == "=":
            take("=")
            value_token = take()
            if not value_token.startswith('"'):
                raise ValueError("Cargo cfg values must be quoted strings")
            return CargoCfgPredicate("atom", name, json.loads(value_token))
        if next_token != "(":
            return CargoCfgPredicate("atom", name)
        if name not in {"all", "any", "not"}:
            raise ValueError(f"unknown Cargo cfg predicate operator: {name}")
        take("(")
        children: list[CargoCfgPredicate] = []
        while position < len(tokens) and tokens[position] != ")":
            children.append(expression(depth + 1))
            if position < len(tokens) and tokens[position] == ")":
                break
            take(",")
        take(")")
        if name == "not" and len(children) != 1:
            raise ValueError("Cargo cfg not requires exactly one predicate")
        if name == "all":
            return CargoCfgPredicate("all", children=tuple(children))
        if name == "any":
            return CargoCfgPredicate("any", children=tuple(children))
        return CargoCfgPredicate("not", children=tuple(children))

    take("cfg")
    take("(")
    result = expression()
    take(")")
    if position != len(tokens):
        raise ValueError(f"trailing Cargo cfg predicate input: {text!r}")
    return result


def parse_rustc_cfg(stdout: str) -> CfgFacts:
    facts: set[tuple[str, str | None]] = set()
    for line in stdout.splitlines():
        if not line.strip():
            continue
        atom = parse_cargo_cfg(f"cfg({line})")
        if atom.kind != "atom":
            raise ValueError("rustc cfg output contains a non-atomic predicate")
        # Cargo's multi-crate-type query removes this user-specific marker.
        if (atom.name, atom.value) != ("proc_macro", None):
            facts.add((atom.name, atom.value))
    if not facts:
        raise ValueError("rustc cfg output contains no target facts")
    return frozenset(facts)


@dataclass(frozen=True, slots=True)
class RustcTargetMetadata:
    sysroot: Path
    cfg: CfgFacts
    crate_filenames: tuple[tuple[str, str | None], ...]
    split_debuginfo: tuple[str, ...]

    def target_libdir(self, target: str) -> Path:
        # Cargo TargetInfo derives this path from the same queried sysroot.
        return self.sysroot / "lib" / "rustlib" / target / "lib"


def cargo_target_query_arguments(
    target: str | None, flags: tuple[str, ...]
) -> tuple[str, ...]:
    """Cargo 0.97 TargetInfo query, including its wrapper-visible envelope."""
    return (
        "-",
        "--crate-name",
        CARGO_QUERY_CRATE_NAME,
        "--print=file-names",
        *flags,
        *(("--target", target) if target is not None else ()),
        *(
            token
            for kind in CARGO_QUERY_CRATE_TYPES
            for token in ("--crate-type", kind)
        ),
        "--print=sysroot",
        "--print=split-debuginfo",
        "--print=crate-name",
        "--print=cfg",
        "-Wwarnings",
    )


def parse_rustc_target_metadata(stdout: str, stderr: str) -> RustcTargetMetadata:
    lines = iter(stdout.splitlines())
    filenames: list[tuple[str, str | None]] = []
    for kind in CARGO_QUERY_CRATE_TYPES:
        unsupported = any(
            ("unsupported crate type" in line or "unknown crate type" in line)
            and f"crate type `{kind}`" in line
            for line in stderr.splitlines()
        )
        if unsupported:
            filenames.append((kind, None))
            continue
        filename = next(lines, None)
        if filename is None or CARGO_QUERY_CRATE_NAME not in filename:
            raise ValueError(
                f"runtime rustc target metadata is missing {kind} file name"
            )
        filenames.append((kind, filename.strip()))
    sysroot_text = next(lines, None)
    if not sysroot_text or not Path(sysroot_text).is_absolute():
        raise ValueError("runtime rustc target metadata is missing an absolute sysroot")
    split_debuginfo: list[str] = []
    for line in lines:
        if line == CARGO_QUERY_CRATE_NAME:
            break
        split_debuginfo.append(line)
    else:
        raise ValueError(
            "runtime rustc target metadata is missing its crate-name delimiter"
        )
    return RustcTargetMetadata(
        Path(sysroot_text),
        parse_rustc_cfg("\n".join(lines)),
        tuple(filenames),
        tuple(split_debuginfo),
    )


def select_cargo_target_flags(
    tables: Mapping[str, Mapping[str, object]],
    *,
    target_flags: tuple[str, ...],
    build_flags: tuple[str, ...],
    environment_flags: tuple[str, ...] | None,
    flags: Callable[[object], tuple[str, ...]],
    probe: Callable[[tuple[str, ...]], CfgFacts],
    transform: Callable[[tuple[str, ...]], tuple[str, ...]] | None = None,
) -> tuple[tuple[str, ...], tuple[tuple[str, Mapping[str, object]], ...]]:
    predicates = tuple(
        (key, parse_cargo_cfg(key), value) for key, value in sorted(tables.items())
    )

    def matching(facts: CfgFacts) -> tuple[tuple[str, Mapping[str, object]], ...]:
        return tuple(
            (key, value)
            for key, predicate, value in predicates
            if predicate.matches(facts)
        )

    def selected(
        matches: tuple[tuple[str, Mapping[str, object]], ...],
    ) -> tuple[str, ...]:
        if environment_flags is not None:
            return environment_flags
        combined = target_flags + tuple(
            token
            for _, table in matches
            if "rustflags" in table
            for token in flags(table["rustflags"])
        )
        # Cargo treats an empty target flag vector as absent, not an override.
        return combined or build_flags

    current = selected(())
    matched: tuple[tuple[str, Mapping[str, object]], ...] = ()
    if predicates:
        for _turn in range(2):
            matched = matching(probe(current))
            next_flags = selected(matched)
            if next_flags == current:
                break
            current = next_flags
        else:
            raise ValueError(
                "runtime Cargo cfg/rustflags selection does not converge in two passes"
            )
    if transform is not None:
        transformed = transform(current)
        if not isinstance(transformed, tuple) or any(
            not isinstance(token, str) or not token or "\x1f" in token
            for token in transformed
        ):
            raise ValueError("runtime resolved Rust flag token is invalid")
        if transformed != current and predicates:
            # The transformed vector is pinned as CARGO_ENCODED_RUSTFLAGS;
            # cfg still controls linker selection, not this explicit override.
            matched = matching(probe(transformed))
        current = transformed
    return current, matched
