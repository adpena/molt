from __future__ import annotations

import ast
from functools import lru_cache
from pathlib import Path
from typing import Collection, Iterable

from molt._intrinsic_symbols import intrinsic_runtime_symbol_name
from molt._runtime_feature_gates import (
    feature_for_symbol,
    runtime_symbol_available_on_target,
    symbol_availability_depends_on_target_os,
    target_os_from_triple,
)
from molt.source_root import compiler_source_root
from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_BY_SPLIT_EXPORT_NAME,
    WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_SPLIT_EXPORT_NAMES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS,
    WASM_IMPORT_REGISTRY,
    WASM_IMPORT_RUNTIME_FEATURES,
    WASM_POLL_TABLE_IMPORTS,
    WASM_RUNTIME_HOST_EXPORTS,
    wasm_import_signature,
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


def _raw_is_cpython_abi_link_import(name: str) -> bool:
    return (
        WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(name)
        == _CPYTHON_ABI_LINK_IMPORT_CLASS
    )


def wasm_split_runtime_canonical_import_name(name: str) -> str:
    return WASM_EXTERNAL_NATIVE_LINK_IMPORT_BY_SPLIT_EXPORT_NAME.get(name, name)


def _is_cpython_abi_link_import(name: str) -> bool:
    return _raw_is_cpython_abi_link_import(
        wasm_split_runtime_canonical_import_name(name)
    )


def _runtime_export_name_or_fail(name: str) -> str:
    name = wasm_split_runtime_canonical_import_name(name)
    # Relocatable runtime artifacts expose CPython ABI link imports under
    # their canonical C names.  The generated runtime export map also records
    # the public ``molt_`` names used by the split shared artifact, so this
    # distinction must be resolved before consulting that map.
    if _is_cpython_abi_link_import(name):
        return name
    export_name = wasm_runtime_export_name(name)
    if export_name is not None:
        return export_name
    raise ValueError(f"unknown WASM runtime import/export name: {name}")


def _split_runtime_export_name_or_fail(name: str) -> str:
    name = wasm_split_runtime_canonical_import_name(name)
    export_name = wasm_runtime_export_name(name)
    if export_name is not None:
        return export_name
    if _is_cpython_abi_link_import(name):
        export_name = WASM_EXTERNAL_NATIVE_LINK_IMPORT_SPLIT_EXPORT_NAMES.get(name)
        if export_name is not None:
            return export_name
    raise ValueError(f"unknown WASM runtime import/export name: {name}")


def wasm_runtime_export_name_for_import(name: str) -> str | None:
    try:
        return _runtime_export_name_or_fail(name)
    except ValueError:
        return None


def wasm_split_runtime_export_name_for_import(name: str) -> str | None:
    try:
        return _split_runtime_export_name_or_fail(name)
    except ValueError:
        return None


def wasm_split_runtime_import_name_for_export(name: str) -> str | None:
    import_name = WASM_EXTERNAL_NATIVE_LINK_IMPORT_BY_SPLIT_EXPORT_NAME.get(name)
    if import_name is not None:
        return import_name
    if _raw_is_cpython_abi_link_import(name):
        return name
    return None


def wasm_split_runtime_import_signature(
    name: str,
) -> tuple[tuple[str, ...], tuple[str, ...]] | None:
    """Return the generated canonical function type for one split ABI edge."""

    canonical_name = wasm_split_runtime_canonical_import_name(name)
    signature = wasm_import_signature(canonical_name)
    if signature is not None:
        return signature
    # Split exports project the canonical env ABI. Preserve its module-qualified
    # identity: an identically named WASI import can have a different signature.
    raw_signature = WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES.get(
        ("env", canonical_name)
    )
    if not isinstance(raw_signature, dict):
        return None
    raw_params = raw_signature.get("params")
    raw_result = raw_signature.get("result")
    if (
        not isinstance(raw_params, list)
        or not all(isinstance(param, str) and param for param in raw_params)
        or not isinstance(raw_result, str)
        or not raw_result
    ):
        raise ValueError(
            f"invalid generated split-runtime signature for {canonical_name}"
        )
    results = () if raw_result == "nil" else tuple(raw_result.split(", "))
    return tuple(raw_params), results


def wasm_split_runtime_export_rename_map(
    required_runtime_imports: Iterable[str] | None,
) -> dict[str, str]:
    if required_runtime_imports is None:
        required_runtime_imports = WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES
    rename_map: dict[str, str] = {}
    for import_name in required_runtime_imports:
        if not _is_cpython_abi_link_import(import_name):
            continue
        export_name = _split_runtime_export_name_or_fail(import_name)
        if export_name != import_name:
            rename_map[import_name] = export_name
    return rename_map


def wasm_static_link_runtime_symbols_for_imports(
    import_symbols: Iterable[str],
    *,
    typed_imports: Iterable[tuple[str, str]] = (),
) -> tuple[str, ...]:
    non_runtime_typed_names = {
        name for module, name in typed_imports if module != "env"
    }
    runtime_symbols: set[str] = set()
    for symbol in import_symbols:
        if symbol in non_runtime_typed_names:
            continue
        try:
            _runtime_export_name_or_fail(symbol)
        except ValueError:
            continue
        runtime_symbols.add(symbol)
    return tuple(sorted(runtime_symbols))


def wasm_cpython_abi_requested_export_names(
    required_runtime_imports: Iterable[str] | None,
) -> tuple[str, ...]:
    if not required_runtime_imports:
        return ()
    return tuple(
        sorted(
            name
            for name in required_runtime_imports
            if _is_cpython_abi_link_import(name) and not name.startswith("molt_")
        )
    )


def wasm_cpython_abi_requested_data_export_names(
    required_runtime_imports: Iterable[str] | None,
) -> tuple[str, ...]:
    return tuple(
        name
        for name in wasm_cpython_abi_requested_export_names(required_runtime_imports)
        if WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS.get(name) == "data"
    )


def wasm_cpython_abi_distribution_export_names() -> tuple[str, ...]:
    """The complete CPython C-API link-import surface of a distributed runtime.

    Distribution uses the same selector as a program's own requested names,
    applied to every canonical link import of the generated WASM ABI manifest.
    Installed programs are still admitted against their own required exports.
    """
    return wasm_cpython_abi_requested_export_names(
        WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES
    )


@lru_cache(maxsize=1)
def wasm_cpython_abi_data_symbol_names() -> tuple[str, ...]:
    """Full, app-independent set of CPython-ABI data symbols the runtime owns.

    These are the canonical singletons and static type/exception objects
    (``Py_None`` / ``Py_True`` / ``Py_False``, every ``PyExc_*``, every
    ``Py*_Type``, ...) that a native extension object (numpy, scipy) references
    as *undefined data symbols*. The split-runtime deploy artifact exports each
    one as an address-bearing wasm global so the linker can point the
    extension's undefined data-symbol references at the runtime's single
    canonical copy (see ``tools/wasm_link.py`` split-runtime data aliasing).

    The set is derived from the generated ABI registry (single authority) and is
    intentionally app-independent so the shared runtime stays byte-identical
    across apps (CDN cacheability, ``test_runtime_hash_identical``).
    """
    return tuple(
        sorted(
            name
            for name, kind in WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS.items()
            if kind == "data" and _raw_is_cpython_abi_link_import(name)
        )
    )


@lru_cache(maxsize=1)
def wasm_runtime_import_names() -> tuple[str, ...]:
    return tuple(sorted(set(WASM_IMPORT_REGISTRY)))


@lru_cache(maxsize=1)
def _wasm_import_registry() -> frozenset[str]:
    return frozenset(WASM_IMPORT_REGISTRY)


def wasm_runtime_import_feature(name: str) -> str | None:
    """The Cargo feature whose runtime build defines import *name*'s export.

    A generated import carries its export's declared feature in the WASM ABI
    projection; any other runtime-owned export is an intrinsic, whose feature
    the intrinsic projection carries. Both project one declaration per symbol.
    """
    canonical = wasm_split_runtime_canonical_import_name(name)
    if canonical in _wasm_import_registry():
        return WASM_IMPORT_RUNTIME_FEATURES.get(canonical)
    symbol = wasm_runtime_export_name_for_import(canonical)
    return None if symbol is None else feature_for_symbol(symbol)


def wasm_runtime_import_names_for_features(
    enabled_features: Collection[str], *, target_triple: str
) -> tuple[str, ...]:
    """The generated runtime imports one runtime build actually defines.

    The registry is the complete ABI. A runtime built with a smaller stdlib
    tier, or for a target without a provider, omits a gated export exactly
    when its declared feature is disabled or the target is excluded; every
    other generated import remains a hard obligation.
    """
    enabled = frozenset(enabled_features)
    selected: list[str] = []
    target_os: str | None = None
    for name in wasm_runtime_import_names():
        symbol = _runtime_export_name_or_fail(name)
        if target_os is None and symbol_availability_depends_on_target_os(symbol):
            target_os = target_os_from_triple(target_triple)
        if not runtime_symbol_available_on_target(
            symbol, target_triple=target_triple, target_os=target_os
        ):
            continue
        feature = WASM_IMPORT_RUNTIME_FEATURES.get(name)
        if feature is not None and feature not in enabled:
            continue
        selected.append(name)
    return tuple(selected)


def wasm_runtime_poll_slot_mask(runtime_import_names: Collection[str]) -> int:
    """The poll-table slots one runtime defines, as the backend's bit mask.

    Bit ``s`` is set when the runtime exports the poll function of generated
    slot ``s``; the backend fills every other slot with its sentinel and does
    not import that function (HF-487: a runtime without a feature lacks its
    poll functions).
    """
    defined = frozenset(runtime_import_names)
    mask = 0
    for slot, name in WASM_POLL_TABLE_IMPORTS:
        if name in defined:
            mask |= 1 << slot
    return mask


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
    repo_root = compiler_source_root()
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


def _all_dynamic_runtime_owned_intrinsic_exports() -> tuple[str, ...]:
    return _all_dynamic_runtime_owned_intrinsic_exports_for_root(compiler_source_root())


@lru_cache(maxsize=8)
def _all_dynamic_runtime_owned_intrinsic_exports_for_root(
    repo_root: Path,
) -> tuple[str, ...]:
    package_root = repo_root / "src" / "molt"
    dynamic_roots = (package_root / "gpu",)
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
            {
                canonical_intrinsic_runtime_name(name)
                for name in _resolved_dynamic_runtime_owned_intrinsic_exports(
                    resolved_modules
                )
            }
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
    for raw_name in required_runtime_imports:
        name = _runtime_export_name_or_fail(raw_name)
        export_names.add(name)
    return tuple(sorted(export_names))


def _runtime_import_symbol_kind(name: str) -> str:
    """Generated symbol-kind authority for one canonical runtime import."""
    canonical = wasm_split_runtime_canonical_import_name(name)
    kind = WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS.get(canonical)
    if kind is None:
        # Manifest runtime imports and dynamic runtime-owned intrinsics are
        # callable functions. CPython-ABI native link imports are the only
        # family that can be generated as address-bearing data symbols.
        return "function"
    if kind not in {"function", "data"}:
        raise ValueError(
            f"unsupported generated WASM runtime symbol kind {kind!r} for {canonical}"
        )
    return kind


def wasm_runtime_required_export_symbol_kinds(
    required_runtime_imports: Iterable[str] | None,
) -> dict[str, str]:
    if not required_runtime_imports:
        return {}
    kinds: dict[str, str] = {}
    for raw_name in required_runtime_imports:
        try:
            export_name = _runtime_export_name_or_fail(raw_name)
        except ValueError:
            continue
        kinds[export_name] = _runtime_import_symbol_kind(raw_name)
    return kinds


def wasm_split_runtime_required_export_symbol_kinds(
    required_runtime_imports: Iterable[str] | None,
) -> dict[str, str]:
    if not required_runtime_imports:
        return {}
    kinds: dict[str, str] = {}
    for raw_name in required_runtime_imports:
        try:
            split_name = _split_runtime_export_name_or_fail(raw_name)
        except ValueError:
            continue
        kinds[split_name] = _runtime_import_symbol_kind(raw_name)
    return kinds


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
        missing.add(name)
    return missing


def wasm_split_runtime_missing_required_exports(
    export_names: Iterable[str],
    required_runtime_imports: Iterable[str] | None,
) -> set[str]:
    if not required_runtime_imports:
        return set()
    available = set(export_names)
    missing: set[str] = set()
    for raw_name in required_runtime_imports:
        try:
            split_name = _split_runtime_export_name_or_fail(raw_name)
        except ValueError:
            missing.add(raw_name)
            continue
        if split_name in available:
            continue
        missing.add(split_name)
    return missing


def _export_if_defined_link_args(export_names: Iterable[str]) -> tuple[str, ...]:
    return tuple(
        token
        for name in sorted(export_names)
        for token in ("-C", f"link-arg=--export-if-defined={name}")
    )


def wasm_runtime_shared_export_link_args(
    required_runtime_imports: Iterable[str] | None = None,
) -> tuple[str, ...]:
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
    # Always publish the canonical CPython-ABI data symbols as address-bearing
    # globals. wasm-ld exports a defined data symbol as an immutable i32 global
    # whose init value is the symbol's linear-memory address; the split-runtime
    # native data-symbol aliaser reads those addresses so numpy/scipy resolve
    # `Py_None`/`Py_False`/`PyExc_*`/`Py*_Type` to the runtime's single copy.
    # `--export-if-defined` keeps this app-independent and never errors on a
    # feature-gated-absent symbol, so the shared artifact stays byte-identical.
    export_names.update(wasm_cpython_abi_data_symbol_names())
    if required_runtime_imports is not None:
        export_names.update(
            wasm_runtime_required_export_names(required_runtime_imports)
        )
    return _export_if_defined_link_args(export_names)


def wasm_runtime_export_link_args(
    required_runtime_imports: Iterable[str] | None = None,
    resolved_modules: Iterable[str] | None = None,
) -> tuple[str, ...]:
    if required_runtime_imports is None:
        return wasm_runtime_shared_export_link_args()
    export_names = set(wasm_runtime_required_export_names(required_runtime_imports))
    export_names.update(wasm_runtime_dynamic_export_names(resolved_modules))
    return _export_if_defined_link_args(export_names)
