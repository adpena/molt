from __future__ import annotations

import re
from functools import lru_cache
from pathlib import Path
from typing import Iterable

from ._wasm_abi_generated import (
    WASM_IMPORT_REGISTRY,
    WASM_RUNTIME_HOST_EXPORTS,
    WASM_RUNTIME_IMPORT_FALLBACK_EXPORTS,
    wasm_runtime_export_name,
    wasm_runtime_import_name,
)

_INTRINSIC_CALL_RE = re.compile(
    r'(?:_(?:require|lazy|optional)_intrinsic|_intrinsic_require)\(\s*"(?P<name>molt_[A-Za-z0-9_]+)"'
    r'|_resolve_optional_intrinsic\(\s*"[^"]+"\s*,\s*"(?P<resolved_name>molt_[A-Za-z0-9_]+)"'
)
_INTRINSIC_SYMBOL_RE = re.compile(
    r'IntrinsicSpec\s*\{\s*name:\s*"(?P<name>[^"]+)"\s*,\s*symbol:\s*"(?P<symbol>[^"]+)"',
    re.DOTALL,
)
def _runtime_export_name_or_fail(name: str) -> str:
    export_name = wasm_runtime_export_name(name)
    if export_name is None:
        raise ValueError(f"unknown WASM runtime import/export name: {name}")
    return export_name


@lru_cache(maxsize=1)
def _runtime_import_fallback_exports() -> dict[str, tuple[str, ...]]:
    fallback_exports: dict[str, tuple[str, ...]] = {}
    for import_name, exports in WASM_RUNTIME_IMPORT_FALLBACK_EXPORTS:
        fallback_exports[_runtime_export_name_or_fail(import_name)] = tuple(exports)
    return fallback_exports


@lru_cache(maxsize=1)
def wasm_runtime_import_names() -> tuple[str, ...]:
    return tuple(sorted(set(WASM_IMPORT_REGISTRY)))


def _runtime_owned_module_path(repo_root: Path, module_name: str) -> Path | None:
    stdlib_root = repo_root / "src" / "molt" / "stdlib"
    if module_name.startswith("molt.stdlib."):
        rel = Path(*module_name[len("molt.stdlib.") :].split("."))
        py_path = (stdlib_root / rel).with_suffix(".py")
        if py_path.exists():
            return py_path
        package_init = stdlib_root / rel / "__init__.py"
        if package_init.exists():
            return package_init
        return None
    if module_name.startswith("molt."):
        package_root = repo_root / "src" / "molt"
        rel = Path(*module_name[len("molt.") :].split("."))
        py_path = (package_root / rel).with_suffix(".py")
        if py_path.exists():
            return py_path
        package_init = package_root / rel / "__init__.py"
        if package_init.exists():
            return package_init
        return None
    rel = Path(*module_name.split("."))
    py_path = (stdlib_root / rel).with_suffix(".py")
    if py_path.exists():
        return py_path
    package_init = stdlib_root / rel / "__init__.py"
    if package_init.exists():
        return package_init
    return None


def _resolved_runtime_owned_intrinsic_exports(
    resolved_modules: Iterable[str] | None,
) -> tuple[str, ...]:
    if not resolved_modules:
        return ()
    repo_root = Path(__file__).resolve().parents[2]
    names: set[str] = set()
    for module_name in resolved_modules:
        module_path = _runtime_owned_module_path(repo_root, module_name)
        if module_path is None:
            continue
        text = module_path.read_text(encoding="utf-8")
        for match in _INTRINSIC_CALL_RE.finditer(text):
            names.add(match.group("name") or match.group("resolved_name"))
    return tuple(sorted(names))


def _resolved_dynamic_runtime_owned_intrinsic_exports(
    resolved_modules: Iterable[str] | None,
) -> tuple[str, ...]:
    if not resolved_modules:
        return ()
    dynamic_modules = tuple(
        module_name
        for module_name in resolved_modules
        if module_name.startswith("molt.")
        and not module_name.startswith("molt.stdlib.")
    )
    return _resolved_runtime_owned_intrinsic_exports(dynamic_modules)


@lru_cache(maxsize=1)
def _all_dynamic_runtime_owned_intrinsic_exports() -> tuple[str, ...]:
    repo_root = Path(__file__).resolve().parents[2]
    package_root = repo_root / "src" / "molt"
    stdlib_root = package_root / "stdlib"
    dynamic_roots = (package_root / "gpu", stdlib_root / "tinygrad")
    names: set[str] = set()
    for root in dynamic_roots:
        if not root.exists():
            continue
        for module_path in sorted(root.rglob("*.py")):
            text = module_path.read_text(encoding="utf-8")
            for match in _INTRINSIC_CALL_RE.finditer(text):
                names.add(match.group("name") or match.group("resolved_name"))
    return tuple(sorted(names))


def wasm_runtime_dynamic_export_names(
    resolved_modules: Iterable[str] | None,
) -> tuple[str, ...]:
    return tuple(
        sorted(
            canonical_intrinsic_runtime_name(name)
            for name in _resolved_dynamic_runtime_owned_intrinsic_exports(
                resolved_modules
            )
        )
    )


@lru_cache(maxsize=1)
def intrinsic_runtime_symbol_names() -> dict[str, str]:
    repo_root = Path(__file__).resolve().parents[2]
    generated_path = (
        repo_root / "runtime" / "molt-runtime" / "src" / "intrinsics" / "generated.rs"
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
    raw_names: set[str] = set()
    for name in _resolved_runtime_owned_intrinsic_exports(resolved_modules):
        import_name = wasm_runtime_import_name(canonical_intrinsic_runtime_name(name))
        if import_name is not None:
            raw_names.add(import_name)
    return tuple(sorted(raw_names))


def wasm_runtime_required_export_names(
    required_runtime_imports: Iterable[str] | None,
) -> tuple[str, ...]:
    if required_runtime_imports is None:
        return tuple(
            sorted(
                _runtime_export_name_or_fail(name)
                for name in wasm_runtime_import_names()
            )
        )
    export_names = set(WASM_RUNTIME_HOST_EXPORTS)
    fallback_exports = _runtime_import_fallback_exports()
    for raw_name in required_runtime_imports:
        name = _runtime_export_name_or_fail(raw_name)
        export_names.add(name)
        export_names.update(fallback_exports.get(name, ()))
    return tuple(sorted(export_names))


def wasm_runtime_missing_required_exports(
    export_names: Iterable[str],
    required_runtime_imports: Iterable[str] | None,
) -> set[str]:
    if not required_runtime_imports:
        return set()
    available = set(export_names)
    missing: set[str] = set()
    for raw_name in required_runtime_imports:
        name = wasm_runtime_export_name(raw_name)
        if name is None:
            missing.add(raw_name)
            continue
        if name in available:
            continue
        fallback_exports = _runtime_import_fallback_exports().get(name)
        if fallback_exports is not None and set(fallback_exports).issubset(available):
            continue
        missing.add(name)
    return missing


def wasm_runtime_export_link_args(
    required_runtime_imports: Iterable[str] | None = None,
    resolved_modules: Iterable[str] | None = None,
) -> str:
    if required_runtime_imports is None:
        export_names = {
            _runtime_export_name_or_fail(name)
            for name in wasm_runtime_import_names()
        }
        export_names.update(WASM_RUNTIME_HOST_EXPORTS)
        export_names.update(
            canonical_intrinsic_runtime_name(name)
            for name in _all_dynamic_runtime_owned_intrinsic_exports()
        )
    else:
        export_names = set(wasm_runtime_required_export_names(required_runtime_imports))
        export_names.update(wasm_runtime_dynamic_export_names(resolved_modules))
    return "".join(
        f" -C link-arg=--export-if-defined={name}" for name in sorted(export_names)
    )
