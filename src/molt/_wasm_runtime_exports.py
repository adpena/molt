from __future__ import annotations

import ast
from functools import lru_cache
from pathlib import Path
from typing import Iterable

from ._intrinsic_symbols import intrinsic_runtime_symbol_name
from ._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_IMPORT_REGISTRY,
    WASM_RUNTIME_HOST_EXPORTS,
    WASM_RUNTIME_IMPORT_FALLBACK_EXPORTS,
    wasm_runtime_export_name,
)

_CPYTHON_ABI_LINK_IMPORT_CLASS = "molt_cpython_abi_link_import"

_INTRINSIC_LOADER_CALL_NAMES = frozenset(
    {
        "_intrinsic_require",
        "_lazy_intrinsic",
        "_load_optional_intrinsic",
        "_optional_intrinsic",
        "_require_intrinsic",
        "_resolve_optional_intrinsic",
        "require_intrinsic",
        "require_optional_intrinsic",
    }
)


def _runtime_export_name_or_fail(name: str) -> str:
    export_name = wasm_runtime_export_name(name)
    if export_name is not None:
        return export_name
    if (
        WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(name)
        == _CPYTHON_ABI_LINK_IMPORT_CLASS
    ):
        return name
    raise ValueError(f"unknown WASM runtime import/export name: {name}")


def wasm_runtime_export_name_for_import(name: str) -> str | None:
    try:
        return _runtime_export_name_or_fail(name)
    except ValueError:
        return None


def wasm_static_link_runtime_symbols_for_imports(
    import_symbols: Iterable[str],
) -> tuple[str, ...]:
    runtime_symbols: set[str] = set()
    for symbol in import_symbols:
        try:
            _runtime_export_name_or_fail(symbol)
        except ValueError:
            continue
        runtime_symbols.add(symbol)
    return tuple(sorted(runtime_symbols))


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


def _intrinsic_loader_name(call: ast.Call) -> str | None:
    if isinstance(call.func, ast.Name):
        return call.func.id
    if isinstance(call.func, ast.Attribute):
        return call.func.attr
    return None


def _runtime_intrinsic_names_from_source(module_path: Path) -> tuple[str, ...]:
    tree = ast.parse(module_path.read_text(encoding="utf-8"), filename=str(module_path))
    names: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        loader_name = _intrinsic_loader_name(node)
        if loader_name not in _INTRINSIC_LOADER_CALL_NAMES:
            continue
        for arg in node.args:
            if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                if arg.value.startswith("molt_"):
                    names.add(arg.value)
    return tuple(sorted(names))


def _resolved_dynamic_runtime_owned_intrinsic_exports(
    resolved_modules: Iterable[str] | None,
) -> tuple[str, ...]:
    if not resolved_modules:
        return ()
    repo_root = Path(__file__).resolve().parents[2]
    dynamic_modules = tuple(
        module_name
        for module_name in resolved_modules
        if module_name.startswith("molt.")
        and not module_name.startswith("molt.stdlib.")
    )
    names: set[str] = set()
    for module_name in dynamic_modules:
        module_path = _runtime_owned_module_path(repo_root, module_name)
        if module_path is None:
            continue
        names.update(_runtime_intrinsic_names_from_source(module_path))
    return tuple(sorted(names))


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
            names.update(_runtime_intrinsic_names_from_source(module_path))
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


def canonical_intrinsic_runtime_name(name: str) -> str:
    return intrinsic_runtime_symbol_name(name)


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
        try:
            name = _runtime_export_name_or_fail(raw_name)
        except ValueError:
            missing.add(raw_name)
            continue
        if name in available:
            continue
        fallback_exports = _runtime_import_fallback_exports().get(name)
        if fallback_exports is not None and set(fallback_exports).issubset(available):
            continue
        missing.add(name)
    return missing


def _export_if_defined_link_args(export_names: Iterable[str]) -> str:
    return "".join(
        f" -C link-arg=--export-if-defined={name}" for name in sorted(export_names)
    )


def wasm_runtime_shared_export_link_args(
    required_runtime_imports: Iterable[str] | None = None,
) -> str:
    """Shared split-runtime export surface plus explicit runtime obligations.

    The shared artifact always publishes the full generated public ABI. Native
    extension objects admitted for a build add exact runtime-backed
    obligations (for example CPython ABI variadic C shim symbols) that are not
    part of the generated import registry, so they must be threaded into the
    link args of the same build the export validator checks.
    """
    export_names = {
        _runtime_export_name_or_fail(name) for name in wasm_runtime_import_names()
    }
    export_names.update(WASM_RUNTIME_HOST_EXPORTS)
    export_names.update(
        canonical_intrinsic_runtime_name(name)
        for name in _all_dynamic_runtime_owned_intrinsic_exports()
    )
    if required_runtime_imports is not None:
        export_names.update(
            wasm_runtime_required_export_names(required_runtime_imports)
        )
    return _export_if_defined_link_args(export_names)


def wasm_runtime_export_link_args(
    required_runtime_imports: Iterable[str] | None = None,
    resolved_modules: Iterable[str] | None = None,
) -> str:
    if required_runtime_imports is None:
        return wasm_runtime_shared_export_link_args()
    export_names = set(wasm_runtime_required_export_names(required_runtime_imports))
    export_names.update(wasm_runtime_dynamic_export_names(resolved_modules))
    return _export_if_defined_link_args(export_names)
