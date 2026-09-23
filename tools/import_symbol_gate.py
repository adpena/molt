#!/usr/bin/env python3
"""Fail closed when a `from molt.x import name` names a symbol that no longer exists.

Authority-extraction arcs move definitions between modules. When a consumer
import is missed, the module still parses, `pytest` collections that never
import it stay green, and the break surfaces only when a proof lane runs
(`molt.scientific_stack_versions` importing a symbol that lived only on an
unlanded branch broke the Pact witness lane on main). This gate resolves every
first-party `from <module> import <name>` statically against the target
module's top-level bindings, so the defect fails at gate time, not proof time.

Static only: no module is executed, so runtime-only packages (compiled-target
stdlib) are checked the same as everything else.
"""

from __future__ import annotations

import argparse
import ast
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_ROOTS: tuple[tuple[str, Path], ...] = (
    ("molt", ROOT / "src" / "molt"),
    ("tools", ROOT / "tools"),
)
SCAN_DIRS: tuple[Path, ...] = (ROOT / "src", ROOT / "tools", ROOT / "tests")
FIRST_PARTY_PREFIXES = ("molt", "tools")
# Compiled-program corpora: `molt` there is the compiled runtime module, not
# the host `src/molt` package, so host bindings are not the authority.
COMPILED_CORPUS_DIRS = frozenset(
    {
        "molt_only",
        "differential",
        "parity",
        "compliance",
        "reference",
        "runtime_compat",
        "wasm_planned",
        "luau",
        "fixtures",
        "benchmarks",
        "e2e",
        "c_extensions",
        "gpu",
        "cloudflare",
    }
)
SKIP_PARTS = frozenset({"__pycache__", "node_modules", ".venv"})


@dataclass(frozen=True)
class Finding:
    path: Path
    line: int
    module: str
    name: str

    def render(self) -> str:
        rel = self.path.relative_to(ROOT).as_posix()
        return f"{rel}:{self.line}: `from {self.module} import {self.name}` has no such binding"


def _module_path(module: str) -> Path | None:
    for prefix, root in SOURCE_ROOTS:
        if module == prefix or module.startswith(prefix + "."):
            parts = module.split(".")[1:]
            base = root.joinpath(*parts) if parts else root
            if (base / "__init__.py").is_file():
                return base / "__init__.py"
            candidate = base.with_suffix(".py")
            if candidate.is_file():
                return candidate
            return None
    return None


def _lazy_reexport_names(path: Path) -> set[str] | None:
    """The keys of a module's finite PEP 562 registry.

    A module that forwards attributes through ``__getattr__`` from a literal
    ``_LAZY_REEXPORTS`` dict has a finite, statically known set of lazy
    bindings; anything else forwarded dynamically is unknown (``None``).
    """
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (SyntaxError, UnicodeDecodeError):
        return None
    for node in tree.body:
        value = None
        if (
            isinstance(node, ast.AnnAssign)
            and isinstance(node.target, ast.Name)
            and node.target.id == "_LAZY_REEXPORTS"
        ):
            value = node.value
        elif isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "_LAZY_REEXPORTS"
            for target in node.targets
        ):
            value = node.value
        if value is None:
            continue
        if not isinstance(value, ast.Dict):
            return None
        keys: set[str] = set()
        for key in value.keys:
            if not (isinstance(key, ast.Constant) and isinstance(key.value, str)):
                return None
            keys.add(key.value)
        return keys
    return None


def _top_level_bindings(path: Path) -> set[str] | None:
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (SyntaxError, UnicodeDecodeError):
        return None
    names: set[str] = set()
    star_import = False
    # Modules whose ``__getattr__`` (PEP 562 forwarding) this module binds:
    # itself when it defines one, or the module it imports one from.
    forwarding_sources: list[Path] = []
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if node.name == "__getattr__":
                forwarding_sources.append(path)
            names.add(node.name)
        elif isinstance(node, ast.Import):
            for alias in node.names:
                names.add((alias.asname or alias.name).split(".")[0])
        elif isinstance(node, ast.ImportFrom):
            for alias in node.names:
                if alias.name == "*":
                    star_import = True
                    continue
                bound = alias.asname or alias.name
                names.add(bound)
                if bound == "__getattr__":
                    source = (
                        _module_path(node.module)
                        if node.module and not node.level
                        else None
                    )
                    forwarding_sources.append(source or path)
        elif isinstance(node, (ast.Assign, ast.AugAssign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                for leaf in ast.walk(target):
                    if isinstance(leaf, ast.Name):
                        names.add(leaf.id)
        elif isinstance(node, (ast.If, ast.Try, ast.With)):
            # Conditional definitions (try/except ImportError, platform gates):
            # collect every binding in the nested bodies conservatively.
            for leaf in ast.walk(node):
                if isinstance(
                    leaf, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
                ):
                    names.add(leaf.name)
                elif isinstance(leaf, ast.Name) and isinstance(leaf.ctx, ast.Store):
                    names.add(leaf.id)
                elif isinstance(leaf, (ast.Import, ast.ImportFrom)):
                    for alias in leaf.names:
                        if alias.name != "*":
                            names.add((alias.asname or alias.name).split(".")[0])
                        else:
                            star_import = True
    if star_import:
        return None
    for source in forwarding_sources:
        lazy = _lazy_reexport_names(source)
        if lazy is None:
            # Dynamic forwarding without a finite registry: bindings unknown.
            return None
        names |= lazy
    return names


def _submodule_exists(module: str, name: str) -> bool:
    return _module_path(f"{module}.{name}") is not None


def scan(paths: tuple[Path, ...] | None = None) -> list[Finding]:
    findings: list[Finding] = []
    if paths is None:
        paths = SCAN_DIRS
    binding_cache: dict[Path, set[str] | None] = {}
    for scan_root in paths:
        for path in sorted(scan_root.rglob("*.py")):
            rel_parts = path.relative_to(ROOT).parts
            if any(part in SKIP_PARTS for part in rel_parts):
                continue
            if rel_parts[0] == "tests" and any(
                part in COMPILED_CORPUS_DIRS for part in rel_parts[1:-1]
            ):
                continue
            try:
                tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            except (SyntaxError, UnicodeDecodeError):
                continue
            for node in ast.walk(tree):
                if (
                    not isinstance(node, ast.ImportFrom)
                    or node.level
                    or node.module is None
                ):
                    continue
                module = node.module
                if module.split(".")[0] not in FIRST_PARTY_PREFIXES:
                    continue
                target = _module_path(module)
                if target is None:
                    continue
                if target not in binding_cache:
                    binding_cache[target] = _top_level_bindings(target)
                bindings = binding_cache[target]
                if bindings is None:
                    continue
                for alias in node.names:
                    if alias.name == "*" or alias.name in bindings:
                        continue
                    if target.name == "__init__.py" and _submodule_exists(
                        module, alias.name
                    ):
                        continue
                    findings.append(Finding(path, node.lineno, module, alias.name))
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "paths",
        nargs="*",
        type=Path,
        help="directories to scan (default: src, tools, tests)",
    )
    args = parser.parse_args(argv)
    paths = tuple(p.resolve() for p in args.paths) if args.paths else SCAN_DIRS
    findings = scan(paths)
    for finding in findings:
        print(finding.render())
    print(
        f"import_symbol_gate: {len(findings)} unresolved first-party from-import name(s)"
    )
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
