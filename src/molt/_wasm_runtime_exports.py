from __future__ import annotations

import re
from functools import lru_cache
from pathlib import Path
from typing import Iterable

_IMPORT_REGISTRY_ENTRY_RE = re.compile(r'\("([^"]+)",\s*\d+\)')
_INTRINSIC_CALL_RE = re.compile(
    r'(?:_(?:require|lazy|optional)_intrinsic|_intrinsic_require)\(\s*"(?P<name>molt_[A-Za-z0-9_]+)"'
)
_INTRINSIC_SYMBOL_RE = re.compile(
    r'IntrinsicSpec\s*\{\s*name:\s*"(?P<name>[^"]+)"\s*,\s*symbol:\s*"(?P<symbol>[^"]+)"',
    re.DOTALL,
)
_HOST_RUNTIME_EXPORTS = frozenset(
    {
        "molt_runtime_shutdown",
        "molt_set_wasm_table_base",
    }
)


@lru_cache(maxsize=1)
def wasm_runtime_import_names() -> tuple[str, ...]:
    repo_root = Path(__file__).resolve().parents[2]
    registry_path = repo_root / "runtime" / "molt-backend" / "src" / "wasm_imports.rs"
    names: list[str] = []
    in_registry = False
    for raw_line in registry_path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not in_registry:
            if line.startswith("pub(crate) const IMPORT_REGISTRY:"):
                in_registry = True
            continue
        if line == "];":
            break
        match = _IMPORT_REGISTRY_ENTRY_RE.search(line)
        if match:
            names.append(match.group(1))
    if not names:
        raise RuntimeError(
            f"failed to read wasm import registry from {registry_path}"
        )
    return tuple(sorted(set(names)))


def _stdlib_module_path(repo_root: Path, module_name: str) -> Path | None:
    stdlib_root = repo_root / "src" / "molt" / "stdlib"
    rel = Path(*module_name.split("."))
    py_path = (stdlib_root / rel).with_suffix(".py")
    if py_path.exists():
        return py_path
    package_init = stdlib_root / rel / "__init__.py"
    if package_init.exists():
        return package_init
    return None


def _resolved_stdlib_intrinsic_exports(
    resolved_modules: Iterable[str] | None,
) -> tuple[str, ...]:
    if not resolved_modules:
        return ()
    repo_root = Path(__file__).resolve().parents[2]
    names: set[str] = set()
    for module_name in resolved_modules:
        module_path = _stdlib_module_path(repo_root, module_name)
        if module_path is None:
            continue
        text = module_path.read_text(encoding="utf-8")
        for match in _INTRINSIC_CALL_RE.finditer(text):
            names.add(match.group("name"))
    return tuple(sorted(names))


@lru_cache(maxsize=1)
def intrinsic_runtime_symbol_names() -> dict[str, str]:
    repo_root = Path(__file__).resolve().parents[2]
    generated_path = (
        repo_root
        / "runtime"
        / "molt-runtime"
        / "src"
        / "intrinsics"
        / "generated.rs"
    )
    text = generated_path.read_text(encoding="utf-8")
    mapping = {
        match.group("name"): match.group("symbol")
        for match in _INTRINSIC_SYMBOL_RE.finditer(text)
    }
    if not mapping:
        raise RuntimeError(
            f"failed to read intrinsic symbol mapping from {generated_path}"
        )
    return mapping


def canonical_intrinsic_runtime_name(name: str) -> str:
    return intrinsic_runtime_symbol_names().get(name, name)


def wasm_runtime_required_import_names(
    resolved_modules: Iterable[str] | None,
) -> tuple[str, ...]:
    raw_names = {
        canonical_intrinsic_runtime_name(name).removeprefix("molt_")
        for name in _resolved_stdlib_intrinsic_exports(resolved_modules)
    }
    known = set(wasm_runtime_import_names())
    return tuple(sorted(raw_names & known))


def wasm_runtime_export_link_args(
    required_runtime_imports: Iterable[str] | None = None,
    resolved_modules: Iterable[str] | None = None,
) -> str:
    if required_runtime_imports is None:
        export_names = {f"molt_{name}" for name in wasm_runtime_import_names()}
    else:
        export_names = set(_HOST_RUNTIME_EXPORTS)
        export_names.update(
            name if name.startswith("molt_") else f"molt_{name}"
            for name in required_runtime_imports
        )
    export_names.update(
        canonical_intrinsic_runtime_name(name)
        for name in _resolved_stdlib_intrinsic_exports(resolved_modules)
    )
    return "".join(
        f" -C link-arg=--export-if-defined={name}"
        for name in sorted(export_names)
    )
