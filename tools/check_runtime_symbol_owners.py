#!/usr/bin/env python3
"""Fail closed on duplicate physical exports across runtime satellite crates.

The root runtime crate still carries feature-off fallback copies for several
stdlib areas while satellite crates carry feature-on authorities. Those pairs
are governed by Cargo feature cfgs and the per-cell owner checks in
`tests/test_wasm_runtime_export_no_mangle.py`.

Satellite crates, however, are linked together in `stdlib_full`. A symbol
defined by two satellites is not a fallback pair; it is a link-time collision
and a split authority. This guard enforces one physical extern "C" export owner
per symbol across `runtime/molt-runtime-*` satellite crates, including literal
`export_name` entries. Macro definitions are templates, not owners; an invoked
export-producing macro requires explicit scanner support and fails closed.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
RUNTIME_ROOT = REPO_ROOT / "runtime"

if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.rust_source_scan import (  # noqa: E402
    _rust_delimiter_ends,
    mask_rust_comments_and_strings,
    mask_rust_test_items,
    rust_source_tokens,
    rust_test_only_source_files,
    scan_memo,
)
from tools.wasm_abi_gen.attached_entries import functions  # noqa: E402


def _without_macro_templates(text: str) -> tuple[str, set[str]]:
    code = mask_rust_comments_and_strings(text)
    ends = _rust_delimiter_ends(code)
    projected = list(text)
    producers = set()
    for match in re.finditer(r"\bmacro_rules!\s+(\w+)\s*([({\[])", code):
        opening = match.end() - 1
        end = ends[opening]
        if re.search(
            r"#\s*\[\s*(?:unsafe\s*\(\s*)?(?:no_mangle|export_name)\b",
            code[opening:end],
        ):
            producers.add(match[1])
        projected[match.start() : end] = [
            "\n" if char == "\n" else " " for char in text[match.start() : end]
        ]
    return "".join(projected), producers


def _c_link_identity(
    attrs: tuple[str, ...], name: str, *, declaration: bool = False
) -> str | None:
    """Read one physical C link identity at an export or extern declaration."""
    unmangled = False
    physical = None
    for attr in attrs:
        tokens = [token.text for token in rust_source_tokens(attr)][2:-1]
        if tokens[:2] == ["unsafe", "("] and tokens[-1:] == [")"]:
            tokens = tokens[2:-1]
        if tokens == ["no_mangle"]:
            unmangled = True
        elif tokens[:2] == ["link_name" if declaration else "export_name", "="]:
            attached = re.fullmatch(
                r'molt_(?:codegen_abi|runtime_core)::attached_symbol!\(("(?:[^"\\]|\\.)*")\)',
                "".join(tokens[2:]),
            )
            if attached is not None:
                # Read the actual Rust macro projection. Generators run in one
                # Python process; an imported Python projection can remain stale
                # after an upstream generator publishes a new schema.
                logical = json.loads(attached[1])
                macro_source = (
                    REPO_ROOT
                    / "runtime/molt-codegen-abi/src/attached_context_generated.rs"
                ).read_text(encoding="utf-8")
                arm = re.search(
                    rf'^\s*\({re.escape(json.dumps(logical))}\)\s*=>\s*\{{\s*("[^"\n]+")\s*\}};',
                    macro_source,
                    re.MULTILINE,
                )
                if arm is None:
                    raise ValueError(f"unclassified attached export on {name}: {attr}")
                physical = json.loads(arm[1])
                continue
            if len(tokens) != 3:
                raise ValueError(f"nonliteral export_name on {name}: {attr}")
            literal = tokens[2]
            raw = re.fullmatch(r'r(#{0,})"(.*)"\1', literal, re.DOTALL)
            try:
                physical = raw[2] if raw else json.loads(literal)
            except (ValueError, TypeError) as error:
                raise ValueError(
                    f"unsupported export_name on {name}: {attr}"
                ) from error
            if not isinstance(physical, str) or not physical:
                raise ValueError(f"invalid export_name on {name}: {attr}")
        elif "export_name" in tokens or "no_mangle" in tokens:
            raise ValueError(f"conditional export attribute on {name}: {attr}")
    return (
        physical if physical is not None else name if unmangled or declaration else None
    )


@dataclass(frozen=True, slots=True)
class SymbolOwner:
    symbol: str
    crate: str
    path: str
    line: int


def _satellite_src_roots(runtime_root: Path) -> Iterable[Path]:
    for src in sorted(runtime_root.glob("molt-runtime-*/src")):
        if src.is_dir():
            yield src


def _is_shipped_source(path: Path) -> bool:
    if path.name == "test_host.rs":
        return False
    return "tests" not in path.parts


def _line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def collect_symbol_owners(
    runtime_root: Path = RUNTIME_ROOT,
) -> dict[str, list[SymbolOwner]]:
    owners: dict[str, list[SymbolOwner]] = defaultdict(list)
    paths = [
        (src_root.parent.name, path)
        for src_root in _satellite_src_roots(runtime_root)
        for path in sorted(src_root.rglob("*.rs"))
        if _is_shipped_source(path)
    ]
    with scan_memo():
        test_only = rust_test_only_source_files(path for _, path in paths)
        sources = []
        producers = set()
        for crate, path in paths:
            if path.resolve() in test_only:
                continue
            text, macros = _without_macro_templates(
                mask_rust_test_items(path.read_text(encoding="utf-8"))
            )
            producers.update(macros)
            sources.append((crate, path, text))
        for crate, path, text in sources:
            rel_path = path.relative_to(runtime_root.parent).as_posix()
            code = mask_rust_comments_and_strings(text)
            for invocation in re.finditer(r"\b(\w+)!\s*[({\[]", code):
                if invocation[1] in producers:
                    raise ValueError(
                        f"{rel_path}:{_line_number(text, invocation.start())}: "
                        f"unsupported export-producing macro {invocation[1]}!"
                    )
            for function in functions(text, include_private=True):
                header = mask_rust_comments_and_strings(
                    text[function.start : function.params_open], preserve_literals=True
                )
                if not re.search(r'\bextern\s+(?:"C"\s+)?fn\b', header):
                    continue
                try:
                    symbol = _c_link_identity(function.attrs, function.name)
                except ValueError as error:
                    raise ValueError(f"{rel_path}: {error}") from error
                if symbol is None:
                    continue
                owners[symbol].append(
                    SymbolOwner(
                        symbol=symbol,
                        crate=crate,
                        path=rel_path,
                        line=_line_number(text, function.attrs_start),
                    )
                )
    return dict(owners)


def find_cross_crate_collisions(
    owners: dict[str, list[SymbolOwner]],
) -> dict[str, list[SymbolOwner]]:
    collisions: dict[str, list[SymbolOwner]] = {}
    for symbol, symbol_owners in sorted(owners.items()):
        crates = {owner.crate for owner in symbol_owners}
        if len(crates) > 1:
            collisions[symbol] = sorted(
                symbol_owners, key=lambda owner: (owner.crate, owner.path, owner.line)
            )
    return collisions


def _json_payload(collisions: dict[str, list[SymbolOwner]]) -> dict[str, object]:
    return {
        "ok": not collisions,
        "collisions": {
            symbol: [
                {
                    "symbol": owner.symbol,
                    "crate": owner.crate,
                    "path": owner.path,
                    "line": owner.line,
                }
                for owner in owners
            ]
            for symbol, owners in collisions.items()
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit machine-readable collision details",
    )
    args = parser.parse_args(argv)

    try:
        collisions = find_cross_crate_collisions(collect_symbol_owners())
    except ValueError as error:
        if args.json:
            print(json.dumps({"ok": False, "error": str(error), "collisions": {}}))
        else:
            print(f"runtime satellite export scan failed: {error}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(_json_payload(collisions), indent=2, sort_keys=True))
    if not collisions:
        if not args.json:
            print("runtime satellite symbol owners OK")
        return 0

    if not args.json:
        print(
            "runtime satellite symbol owner collision(s) found:",
            file=sys.stderr,
        )
        for symbol, owners in collisions.items():
            print(f"- {symbol}", file=sys.stderr)
            for owner in owners:
                print(f"  {owner.crate}: {owner.path}:{owner.line}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
