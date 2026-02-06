#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
from collections import deque
from pathlib import Path

import check_stdlib_intrinsics as stdlib_audit

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORE_MANIFEST = ROOT / "tests" / "differential" / "core" / "TESTS.txt"


def _canonical_module(name: str, known: set[str]) -> str | None:
    if name in known:
        return name
    root = name.split(".", 1)[0]
    if root in known:
        return root
    return None


def _module_package_parts(module: str, path: Path) -> list[str]:
    if module == "molt.stdlib":
        return []
    parts = module.split(".")
    if path.name == "__init__.py":
        return parts
    return parts[:-1]


def _resolve_from_target(
    *,
    current_module: str,
    current_path: Path,
    level: int,
    module: str | None,
    name: str | None,
) -> str | None:
    package_parts = _module_package_parts(current_module, current_path)
    if level <= 0:
        if module is None:
            return name
        if name is None:
            return module
        return f"{module}.{name}"
    if level > len(package_parts) + 1:
        return None
    anchor = package_parts[: len(package_parts) - level + 1]
    suffix: list[str] = []
    if module:
        suffix.extend(module.split("."))
    if name:
        suffix.append(name)
    parts = anchor + suffix
    if not parts:
        return None
    return ".".join(parts)


def _imports_from_ast(
    *,
    tree: ast.AST,
    current_module: str,
    current_path: Path,
    known_modules: set[str],
) -> set[str]:
    out: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                resolved = _canonical_module(alias.name, known_modules)
                if resolved:
                    out.add(resolved)
        elif isinstance(node, ast.ImportFrom):
            module_name = _resolve_from_target(
                current_module=current_module,
                current_path=current_path,
                level=node.level,
                module=node.module,
                name=None,
            )
            if module_name:
                resolved = _canonical_module(module_name, known_modules)
                if resolved:
                    out.add(resolved)
            for alias in node.names:
                if alias.name == "*":
                    continue
                target = _resolve_from_target(
                    current_module=current_module,
                    current_path=current_path,
                    level=node.level,
                    module=node.module,
                    name=alias.name,
                )
                if not target:
                    continue
                resolved = _canonical_module(target, known_modules)
                if resolved:
                    out.add(resolved)
    return out


def _load_stdlib_audit() -> dict[str, tuple[str, Path]]:
    known_intrinsics = stdlib_audit._load_manifest_intrinsics()
    out: dict[str, tuple[str, Path]] = {}
    failures: list[str] = []
    for path in sorted(stdlib_audit.STDLIB_ROOT.rglob("*.py")):
        errors, intrinsic_names, status, has_stdlib_todo = stdlib_audit._scan_file(path)
        module = stdlib_audit._module_name(path)
        if status == stdlib_audit.STATUS_INTRINSIC and has_stdlib_todo:
            status = stdlib_audit.STATUS_INTRINSIC_PARTIAL
        if errors:
            rel = path.relative_to(ROOT)
            failures.extend(f"{rel}: {msg}" for msg in errors)
        for name in intrinsic_names:
            if name not in known_intrinsics:
                rel = path.relative_to(ROOT)
                failures.append(
                    f"{rel}: intrinsic `{name}` not present in "
                    f"{stdlib_audit.MANIFEST.relative_to(ROOT)}"
                )
        out[module] = (status, path)
    if failures:
        joined = "\n- ".join(sorted(set(failures)))
        raise RuntimeError(f"stdlib audit failed:\n- {joined}")
    return out


def _collect_manifest_tests(manifest: Path) -> list[Path]:
    if not manifest.is_file():
        raise FileNotFoundError(f"Manifest not found: {manifest}")
    tests: list[Path] = []
    for raw in manifest.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        path = Path(line)
        if not path.is_absolute():
            path = ROOT / path
        if not path.exists():
            raise FileNotFoundError(f"Manifest entry missing: {line} ({manifest})")
        tests.append(path)
    return tests


def _collect_seed_modules(manifest: Path, known_modules: set[str]) -> set[str]:
    seeds: set[str] = set()
    for test_path in _collect_manifest_tests(manifest):
        if test_path.suffix != ".py":
            continue
        tree = ast.parse(test_path.read_text(encoding="utf-8"), filename=str(test_path))
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    resolved = _canonical_module(alias.name, known_modules)
                    if resolved:
                        seeds.add(resolved)
            elif isinstance(node, ast.ImportFrom):
                if node.level != 0 or not node.module:
                    continue
                resolved = _canonical_module(node.module, known_modules)
                if resolved:
                    seeds.add(resolved)
    return seeds


def _build_stdlib_dep_graph(
    audit_map: dict[str, tuple[str, Path]],
) -> dict[str, set[str]]:
    known_modules = set(audit_map)
    deps: dict[str, set[str]] = {}
    for module, (_, path) in audit_map.items():
        text = path.read_text(encoding="utf-8")
        tree = ast.parse(text, filename=str(path))
        deps[module] = _imports_from_ast(
            tree=tree,
            current_module=module,
            current_path=path,
            known_modules=known_modules,
        )
    return deps


def _closure(seeds: set[str], deps: dict[str, set[str]]) -> set[str]:
    seen: set[str] = set()
    queue: deque[str] = deque(sorted(seeds))
    while queue:
        module = queue.popleft()
        if module in seen:
            continue
        seen.add(module)
        for dep in sorted(deps.get(module, ())):
            if dep not in seen:
                queue.append(dep)
    return seen


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Ensure compiled core-lane imports resolve only to fully lowered "
            "stdlib modules."
        )
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_CORE_MANIFEST,
        help="Path to a differential lane manifest (default: core lane).",
    )
    parser.add_argument(
        "--allow-status",
        action="append",
        default=[],
        help=(
            "Allowed audit status (repeatable). Default is " "`intrinsic-backed` only."
        ),
    )
    args = parser.parse_args()

    allow_statuses = set(args.allow_status) or {stdlib_audit.STATUS_INTRINSIC}
    audit_map = _load_stdlib_audit()
    dep_graph = _build_stdlib_dep_graph(audit_map)
    seed_modules = _collect_seed_modules(args.manifest, set(audit_map))

    if not seed_modules:
        print(f"core-lane lowering gate: no stdlib imports in {args.manifest}")
        return 0

    imported_closure = _closure(seed_modules, dep_graph)
    violations: list[tuple[str, str, Path]] = []
    for module in sorted(imported_closure):
        status, path = audit_map[module]
        if status not in allow_statuses:
            violations.append((module, status, path))

    if violations:
        print("core-lane lowering gate failed:")
        print(f"- manifest: {args.manifest}")
        print(f"- allowed statuses: {', '.join(sorted(allow_statuses))}")
        print("- violating modules:")
        for module, status, path in violations:
            print(f"  - {module}: {status} ({path.relative_to(ROOT)})")
        return 1

    print("core-lane lowering gate: ok")
    print(f"- manifest: {args.manifest}")
    print(f"- seed modules: {', '.join(sorted(seed_modules))}")
    print(f"- closure size: {len(imported_closure)} modules")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
