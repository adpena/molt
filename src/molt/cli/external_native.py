from __future__ import annotations

import contextlib
import json
import os
import re
from functools import lru_cache
from collections.abc import Collection, Mapping, Sequence
from pathlib import Path
from typing import Any, cast

from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_IMPORT_REGISTRY,
    WASM_LINK_ALLOWED_IMPORTS,
    WASM_LINK_ALLOWED_IMPORT_PRIMITIVE_CLASSES,
    WASM_RUNTIME_HOST_EXPORTS,
    wasm_runtime_export_name,
)
from molt.cli.atomic_io import _atomic_copy_file, _remove_file_or_tree
from molt.cli.c_api_symbols import c_api_primitive_class
from molt.cli.c_api_symbols import is_c_api_external_requirement
from molt.cli.c_api_symbols import is_c_api_symbol
from molt.cli.extension_manifest import (
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _host_target_triple,
    _py_methoddef_names,
    _validate_extension_manifest,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.models import (
    _BuildOutputLayout,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ExternalPackageNativeArtifact,
    _ExternalPackageNativeArtifactPlan,
    _ExternalNativeAbiSymbol,
    _ExternalNativeCallableExport,
    _ExternalNativeCapiSymbol,
    _ImportAdmissionPolicy,
    _StagedExternalPackageNativeArtifact,
)
from molt.cli.module_resolution import _case_exact_file
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.source_extensions import (
    source_extension_manifest_errors_are_missing_sources,
    source_extension_manifest_required_capsule_imports,
)
from molt.wasm_artifact import read_wasm_function_exports, read_wasm_imports


def _parse_external_static_packages(raw: str) -> tuple[frozenset[str], str | None]:
    packages: set[str] = set()
    for part in re.split(r"[\s,]+", raw.strip()):
        if not part:
            continue
        if not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*",
            part,
        ):
            return frozenset(), (
                "MOLT_EXTERNAL_STATIC_PACKAGES must contain comma/space-separated "
                f"Python package names; invalid entry: {part!r}"
            )
        packages.add(part)
    return frozenset(packages), None


_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES = (".so", ".pyd")
_EXTERNAL_PACKAGE_STATIC_LINK_ARTIFACT_SUFFIXES = (".molt.wasm", ".o", ".a")
_EXTERNAL_PACKAGE_ARTIFACT_SUFFIXES = (
    *_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES,
    *_EXTERNAL_PACKAGE_STATIC_LINK_ARTIFACT_SUFFIXES,
)
_EXTERNAL_PACKAGE_STATIC_SUPPORT_SUFFIXES = (".molt.wasm", ".o", ".a", ".py")
_EXTERNAL_PACKAGE_NATIVE_SOURCE_SUFFIXES = (
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".f",
    ".f90",
    ".f95",
    ".pxd",
    ".pyx",
    ".rs",
)
_SOURCE_RECOMPILED_EXTERNAL_PACKAGE_ROOTS = frozenset({"numpy", "scipy"})
_EXTERNAL_PACKAGE_EXTENSION_MANIFEST = "extension_manifest.json"
_EXTERNAL_PACKAGE_ARTIFACT_MANIFEST_SUFFIX = ".extension_manifest.json"
_WASM_CPYTHON_ABI_LINK_IMPORT_PREFIXES = ("molt_cpython_abi_",)


_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_EXCLUDED_DIRS = {
    ".git",
    ".hg",
    ".mypy_cache",
    ".nox",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "__pycache__",
    "build",
    "dist",
    "node_modules",
    "site-packages",
}


def _external_package_dir(root: Path, package: str) -> Path | None:
    package_dir = root.joinpath(*package.split("."))
    init_file = package_dir / "__init__.py"
    if package_dir.is_dir() and _case_exact_file(init_file):
        return package_dir.resolve()
    return None


def _source_recompiled_external_package_root(package: str) -> str | None:
    normalized = package.strip()
    for root in sorted(_SOURCE_RECOMPILED_EXTERNAL_PACKAGE_ROOTS):
        if normalized == root or normalized.startswith(root + "."):
            return root
    return None


def _native_artifact_source_packages_for_admission(
    admitted_packages: Collection[str],
) -> frozenset[str]:
    return frozenset(
        root
        for package in admitted_packages
        if (root := _source_recompiled_external_package_root(package)) is not None
    )


def _is_external_package_native_artifact(path: Path) -> bool:
    name = path.name.lower()
    return any(
        name.endswith(suffix) for suffix in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES
    )


def _is_external_package_static_link_artifact(path: Path) -> bool:
    name = path.name.lower()
    return any(
        name.endswith(suffix)
        for suffix in _EXTERNAL_PACKAGE_STATIC_LINK_ARTIFACT_SUFFIXES
    )


def _external_artifact_manifest_path(artifact_path: Path) -> Path:
    return artifact_path.with_name(
        artifact_path.name + _EXTERNAL_PACKAGE_ARTIFACT_MANIFEST_SUFFIX
    )


def _is_external_extension_manifest_filename(filename: str) -> bool:
    lowered = filename.lower()
    return lowered == _EXTERNAL_PACKAGE_EXTENSION_MANIFEST or lowered.endswith(
        _EXTERNAL_PACKAGE_ARTIFACT_MANIFEST_SUFFIX
    )


def _manifest_declared_static_link_artifact(
    *,
    manifest_path: Path,
    package_dir: Path,
) -> Path | None:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(manifest, dict):
        return None
    if manifest.get("loader_kind") != "libmolt_source":
        return None
    if manifest.get("runtime_linkage") != "static_link":
        return None
    extension = manifest.get("extension")
    if not isinstance(extension, str) or not extension.strip():
        return None
    extension_path = Path(extension.strip())
    candidates = (
        (extension_path,)
        if extension_path.is_absolute()
        else (manifest_path.parent / extension_path, package_dir / extension_path)
    )
    package_root = package_dir.resolve()
    for candidate in candidates:
        resolved = candidate.resolve()
        if not (resolved == package_root or resolved.is_relative_to(package_root)):
            continue
        if _case_exact_file(resolved) and _is_external_package_static_link_artifact(
            resolved
        ):
            return resolved
    return None


def _iter_external_package_native_artifacts(package_dir: Path) -> list[Path]:
    artifacts: dict[Path, None] = {}
    for current_root, dirnames, filenames in os.walk(package_dir):
        dirnames[:] = sorted(
            dirname
            for dirname in dirnames
            if dirname not in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_EXCLUDED_DIRS
            and not (Path(current_root) / dirname).is_symlink()
        )
        current = Path(current_root)
        for filename in sorted(filenames):
            path = current / filename
            if path.is_symlink():
                continue
            if _is_external_package_native_artifact(path):
                artifacts[path.resolve()] = None
                continue
            if _is_external_extension_manifest_filename(filename):
                declared = _manifest_declared_static_link_artifact(
                    manifest_path=path,
                    package_dir=package_dir,
                )
                if declared is not None:
                    artifacts[declared] = None
    return sorted(artifacts)


def _external_package_native_artifact_candidate_errors(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
) -> list[str]:
    errors: list[str] = []
    for package in sorted(admitted_packages):
        native_root = _source_recompiled_external_package_root(package)
        if native_root is None:
            continue
        package_dirs: list[Path] = []
        artifact_candidate_count = 0
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            package_dirs.append(package_dir)
            artifact_candidate_count += len(
                _iter_external_package_native_artifacts(package_dir)
            )
        if not package_dirs or artifact_candidate_count:
            continue
        roots = ", ".join(str(path) for path in package_dirs)
        errors.append(
            f"{package}: source-recompiled external static package admission "
            f"for native package root {native_root!r} requires at least one "
            "package-local Molt native artifact candidate before graph admission; "
            f"found none under {roots}. Build or stage libmolt source-recompiled "
            f"{native_root} artifacts, or remove {package!r} from "
            "MOLT_EXTERNAL_STATIC_PACKAGES."
        )
    return errors


def _external_extension_module_name(
    *,
    package: str,
    package_dir: Path,
    artifact_path: Path,
) -> str:
    rel = artifact_path.resolve().relative_to(package_dir.resolve())
    parent_parts = rel.parent.parts
    basename = rel.name
    for suffix in _EXTERNAL_PACKAGE_ARTIFACT_SUFFIXES:
        if basename.lower().endswith(suffix):
            basename = basename[: -len(suffix)]
            break
    basename = basename.split(".", 1)[0]
    return ".".join(part for part in (package, *parent_parts, basename) if part)


def _external_native_artifact_module_required(
    *,
    package: str,
    module_name: str,
    required_modules: frozenset[str] | None,
) -> bool:
    if required_modules is None:
        return True
    package_root = package.strip()
    for required_module in required_modules:
        if module_name == required_module or required_module.startswith(
            module_name + "."
        ):
            return True
        if required_module != package_root and module_name.startswith(
            required_module + "."
        ):
            return True
    return False


def _external_native_provider_reaches_required(
    *,
    provider_name: str,
    required_name: str,
) -> bool:
    return required_name == provider_name or required_name.startswith(
        provider_name + "."
    )


def _extension_path_matches_manifest(
    *,
    path: Path,
    manifest_extension: str,
    manifest_dir: Path,
    package_dir: Path,
) -> bool:
    expected_norm = manifest_extension.replace("\\", "/").strip()
    if not expected_norm:
        return False
    artifact_path = path.resolve()
    manifest_path = Path(expected_norm)
    if manifest_path.is_absolute():
        return manifest_path.resolve() == artifact_path
    return (manifest_dir / manifest_path).resolve() == artifact_path or (
        package_dir / manifest_path
    ).resolve() == artifact_path


def _find_external_extension_manifest(
    *,
    artifact_path: Path,
    package_dir: Path,
) -> Path | None:
    package_root = package_dir.resolve()
    artifact_specific = _external_artifact_manifest_path(artifact_path.resolve())
    if artifact_specific.parent == artifact_path.resolve().parent and _case_exact_file(
        artifact_specific
    ):
        return artifact_specific.resolve()
    current = artifact_path.resolve().parent
    for _ in range(6):
        if not (current == package_root or current.is_relative_to(package_root)):
            return None
        candidate = current / _EXTERNAL_PACKAGE_EXTENSION_MANIFEST
        if _case_exact_file(candidate):
            return candidate.resolve()
        if current == package_root:
            return None
        current = current.parent
    return None


def _required_manifest_str(
    manifest: Mapping[str, Any],
    field_name: str,
    errors: list[str],
) -> str:
    value = manifest.get(field_name)
    if isinstance(value, str) and value.strip():
        return value.strip()
    errors.append(f"extension_manifest.json {field_name} must be a non-empty string")
    return ""


def _manifest_support_file_sha256(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
    package_source_root: Path,
    errors: list[str],
) -> tuple[tuple[str, str], ...]:
    raw_support_files = manifest.get("support_files")
    if raw_support_files is None:
        return ()
    if not isinstance(raw_support_files, list):
        errors.append("extension_manifest.json support_files must be a list")
        return ()
    out: list[tuple[str, str]] = []
    seen: set[str] = set()
    for index, item in enumerate(raw_support_files):
        label = f"extension_manifest.json support_files[{index}]"
        if not isinstance(item, Mapping):
            errors.append(f"{label} must be an object")
            continue
        rel_value = item.get("path")
        sha_value = item.get("sha256")
        if not isinstance(rel_value, str) or not rel_value.strip():
            errors.append(f"{label}.path must be a non-empty relative path")
            continue
        rel_text = rel_value.replace("\\", "/").strip()
        rel_path = Path(rel_text)
        if rel_path.is_absolute() or any(
            part in {"", ".", ".."} for part in rel_path.parts
        ):
            errors.append(f"{label}.path escapes the package root: {rel_value!r}")
            continue
        if not (
            rel_text.endswith(".molt.wasm")
            or Path(rel_text).suffix in _EXTERNAL_PACKAGE_STATIC_SUPPORT_SUFFIXES
        ):
            errors.append(
                f"{label}.path must name a wasm static-link support artifact "
                "(.molt.wasm, .o, or .a) or checksummed upstream Python source "
                "(.py)"
            )
            continue
        if not isinstance(sha_value, str) or not re.fullmatch(
            r"[0-9a-fA-F]{64}", sha_value.strip()
        ):
            errors.append(f"{label}.sha256 must be a lowercase SHA-256 hex digest")
            continue
        if rel_text in seen:
            errors.append(f"{label}.path duplicates support file {rel_text!r}")
            continue
        support_path = (package_source_root / rel_path).resolve()
        try:
            support_path.relative_to(package_source_root.resolve())
        except ValueError:
            errors.append(f"{label}.path escapes the package root: {rel_value!r}")
            continue
        if not support_path.is_file():
            errors.append(
                f"{label}.path does not exist relative to {manifest_path.parent}: "
                f"{rel_text}"
            )
            continue
        expected = sha_value.strip().lower()
        actual = _sha256_file(support_path).lower()
        if actual != expected:
            errors.append(
                f"{label}.sha256 mismatch for {rel_text}: expected {expected}, got {actual}"
            )
            continue
        seen.add(rel_text)
        out.append((rel_text, expected))
    return tuple(sorted(out))


def _support_python_module_names(
    support_file_sha256: Collection[tuple[str, str]],
) -> frozenset[str]:
    names: set[str] = set()
    for rel_path, _digest in support_file_sha256:
        normalized = rel_path.replace("\\", "/")
        if not normalized.endswith(".py"):
            continue
        parts = normalized.split("/")
        if not parts:
            continue
        if parts[-1] == "__init__.py":
            module_parts = parts[:-1]
        else:
            module_parts = [*parts[:-1], parts[-1][:-3]]
        if module_parts:
            names.add(".".join(module_parts))
    return frozenset(names)


def _manifest_source_paths(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
    errors: list[str],
) -> tuple[Path, ...]:
    raw_sources = manifest.get("sources")
    if raw_sources is None:
        return ()
    if not isinstance(raw_sources, list) or not all(
        isinstance(item, str) and item.strip() for item in raw_sources
    ):
        errors.append("extension_manifest.json sources must be a list of paths")
        return ()
    source_entries = cast(list[str], raw_sources)
    paths: list[Path] = []
    for index, raw_source in enumerate(source_entries):
        source_path = Path(raw_source).expanduser()
        if not source_path.is_absolute():
            source_path = (manifest_path.parent / source_path).resolve()
        if not source_path.is_file():
            errors.append(
                f"extension_manifest.json sources[{index}] does not exist: "
                f"{source_path}"
            )
            continue
        paths.append(source_path)
    return tuple(paths)


def _manifest_py_methoddef_names(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
    errors: list[str],
) -> frozenset[str]:
    names: set[str] = set()
    for source_path in _manifest_source_paths(
        manifest,
        manifest_path=manifest_path,
        errors=errors,
    ):
        try:
            names.update(
                _py_methoddef_names(
                    source_path.read_text(encoding="utf-8", errors="replace")
                )
            )
        except OSError as exc:
            errors.append(
                f"extension_manifest.json source {source_path} could not be read: {exc}"
            )
    return frozenset(names)


def _validate_module_attr_callable_export_custody(
    *,
    package: str,
    manifest: Mapping[str, Any],
    manifest_path: Path,
    module_name: str,
    callable_exports: Collection["_ExternalNativeCallableExport"],
    support_file_sha256: Collection[tuple[str, str]],
) -> list[str]:
    errors: list[str] = []
    module_attr_exports = [
        export for export in callable_exports if export.binding == "module_attr"
    ]
    if not module_attr_exports:
        return []

    support_modules = _support_python_module_names(support_file_sha256)
    method_names: frozenset[str] | None = None
    for export in module_attr_exports:
        provider_module = export.provider_module or module_name
        if provider_module == module_name:
            if method_names is None:
                method_errors: list[str] = []
                method_names = _manifest_py_methoddef_names(
                    manifest,
                    manifest_path=manifest_path,
                    errors=method_errors,
                )
                errors.extend(f"{package}: {error}" for error in method_errors)
            if export.name not in method_names:
                errors.append(
                    f"{package}: callable export {export.qualified_name!r} uses "
                    f"module_attr provider {module_name!r}, but {export.name!r} "
                    "is not declared by a PyMethodDef entry in the admitted "
                    "extension sources. Declare an explicit provider_module backed "
                    "by checksummed upstream Python support source, or publish an "
                    "actual native extension method/direct_symbol export."
                )
            continue
        if provider_module not in support_modules:
            errors.append(
                f"{package}: callable export {export.qualified_name!r} uses "
                f"provider_module {provider_module!r}, but that provider is not "
                "a checksummed .py entry in extension_manifest.json support_files. "
                "Native wrapper publication must be backed by admitted upstream "
                "support source instead of package ancestry."
            )
    return errors


def _manifest_str_tuple(
    manifest: Mapping[str, Any],
    field_name: str,
) -> tuple[str, ...]:
    value = manifest.get(field_name)
    if not isinstance(value, list):
        return ()
    return tuple(
        sorted(
            {item.strip() for item in value if isinstance(item, str) and item.strip()}
        )
    )


def _manifest_object_closure_required_capsules(
    manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    required = set(_manifest_str_tuple(object_closure, "required_capsules"))
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if isinstance(item, Mapping):
                required.update(_manifest_str_tuple(item, "required_capsules"))
    return tuple(sorted(required))


def _manifest_has_sealed_extension_custody(manifest: Mapping[str, Any]) -> bool:
    object_closure = manifest.get("object_closure")
    return (
        isinstance(object_closure, Mapping)
        and isinstance(manifest.get("sealed_from_manifest_sha256"), str)
        and isinstance(manifest.get("sealed_from_extension_sha256"), str)
    )


def _validate_manifest_source_capsule_requirements(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
) -> list[str]:
    source_required, errors = source_extension_manifest_required_capsule_imports(
        manifest,
        manifest_path=manifest_path,
    )
    if errors:
        if _manifest_has_sealed_extension_custody(
            manifest
        ) and source_extension_manifest_errors_are_missing_sources(errors):
            return []
        return errors
    assert source_required is not None
    declared = set(_manifest_object_closure_required_capsules(manifest))
    missing = sorted(set(source_required) - declared)
    if not missing:
        return []
    details = ", ".join(
        f"{capsule} via {', '.join(source_required[capsule])}" for capsule in missing
    )
    return [
        "source-derived capsule requirement(s) missing from "
        f"object_closure.required_capsules: {details}. Regenerate the extension "
        "manifest from source scan authority before admitting this native artifact."
    ]


def _manifest_object_closure_defined_symbols(
    manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    defined: set[str] = set()
    raw_symbols = object_closure.get("defined_symbols")
    if isinstance(raw_symbols, list):
        defined.update(
            symbol.strip()
            for symbol in raw_symbols
            if isinstance(symbol, str) and symbol.strip()
        )
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if isinstance(item, Mapping):
                defined.update(_manifest_str_tuple(item, "defined_symbols"))
    return tuple(sorted(defined))


def _manifest_object_closure_runtime_symbols(
    manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    runtime_symbols = set(_manifest_str_tuple(object_closure, "runtime_symbols"))
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if isinstance(item, Mapping):
                runtime_symbols.update(_manifest_str_tuple(item, "runtime_symbols"))
    return tuple(sorted(runtime_symbols))


def _manifest_object_closure_required_c_api_symbols(
    manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    required = {
        symbol
        for symbol in _manifest_str_tuple(object_closure, "required_c_api_symbols")
        if is_c_api_external_requirement(symbol)
    }
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if isinstance(item, Mapping):
                required.update(
                    symbol
                    for symbol in _manifest_str_tuple(item, "required_c_api_symbols")
                    if is_c_api_external_requirement(symbol)
                )
    return tuple(sorted(required))


def _manifest_object_closure_undefined_symbols(
    manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    undefined = set(_manifest_str_tuple(object_closure, "undefined_symbols"))
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if isinstance(item, Mapping):
                undefined.update(_manifest_str_tuple(item, "undefined_symbols"))
    return tuple(sorted(undefined))


def _manifest_object_closure_external_undefined_symbols(
    manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    return _manifest_str_tuple(object_closure, "undefined_symbols")


def _molt_root_for_external_native_scan() -> Path:
    return Path(__file__).resolve().parents[3]


def _c_api_scan_header_for_manifest(
    manifest: Mapping[str, Any],
    *,
    molt_root: Path,
) -> Path | None:
    if manifest.get("abi_tier") == "cpython-abi":
        return molt_root / "runtime" / "molt-cpython-abi" / "include" / "Python.h"
    return None


@lru_cache(maxsize=1)
def _wasm_runtime_backed_abi_symbols() -> frozenset[str]:
    symbols = set(WASM_LINK_ALLOWED_IMPORTS)
    symbols.update(WASM_RUNTIME_HOST_EXPORTS)
    for import_name in WASM_IMPORT_REGISTRY:
        symbols.add(import_name)
        export_name = wasm_runtime_export_name(import_name)
        if export_name is not None:
            symbols.add(export_name)
    return frozenset(symbols)


def _abi_primitive_class(symbol: str) -> str:
    if symbol in WASM_LINK_ALLOWED_IMPORTS:
        return WASM_LINK_ALLOWED_IMPORT_PRIMITIVE_CLASSES[symbol]
    if symbol in WASM_IMPORT_REGISTRY or wasm_runtime_export_name(symbol) is not None:
        return "wasm_runtime_import"
    if symbol in WASM_RUNTIME_HOST_EXPORTS:
        return "wasm_runtime_host_export"
    return "native_project_symbol"


def _external_link_primitive_class(symbol: str) -> str:
    primitive_class = WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(symbol)
    if primitive_class is not None:
        return primitive_class
    if symbol.startswith(_WASM_CPYTHON_ABI_LINK_IMPORT_PREFIXES):
        return "molt_cpython_abi_link_import"
    raise AssertionError(f"unknown external native link import: {symbol!r}")


def _is_external_link_import(symbol: str) -> bool:
    return (
        symbol in WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES
        or symbol.startswith(_WASM_CPYTHON_ABI_LINK_IMPORT_PREFIXES)
    )


def _wasm_static_link_runtime_symbols_for_imports(
    import_symbols: Collection[str],
) -> tuple[str, ...]:
    runtime_backed_symbols = _wasm_runtime_backed_abi_symbols()
    return tuple(
        sorted(
            symbol
            for symbol in import_symbols
            if symbol in runtime_backed_symbols
            or _is_external_link_import(symbol)
            or is_c_api_symbol(symbol)
        )
    )


def _molt_runtime_namespace_symbol(symbol: str) -> bool:
    return symbol.startswith(("molt_", "__molt"))


def _wasm_relocatable_import_symbols(
    *,
    package: str,
    artifact_path: Path,
    artifact_kind: str,
    context: str,
) -> tuple[tuple[str, ...] | None, list[str]]:
    if artifact_kind != "wasm_relocatable_object":
        return None, []
    try:
        return (
            tuple(
                sorted(
                    {
                        wasm_import.name
                        for wasm_import in read_wasm_imports(artifact_path)
                    }
                )
            ),
            [],
        )
    except (OSError, UnicodeDecodeError, ValueError, IndexError) as exc:
        return None, [
            f"{package}: cannot validate {context} for {artifact_path.name}: {exc}"
        ]


def _object_closure_abi_symbol_board(
    manifest: Mapping[str, Any],
    *,
    external_symbols: Collection[str] | None = None,
) -> tuple[tuple[_ExternalNativeAbiSymbol, ...], list[str]]:
    undefined_symbols = set(
        external_symbols
        if external_symbols is not None
        else _manifest_object_closure_undefined_symbols(manifest)
    )
    non_c_api_symbols = sorted(
        symbol for symbol in undefined_symbols if not is_c_api_symbol(symbol)
    )
    if not non_c_api_symbols:
        return (), []

    defined_symbols = set(_manifest_object_closure_defined_symbols(manifest))
    runtime_symbols = set(_manifest_object_closure_runtime_symbols(manifest))
    runtime_backed_symbols = _wasm_runtime_backed_abi_symbols()
    records: list[_ExternalNativeAbiSymbol] = []
    errors: list[str] = []
    for symbol in non_c_api_symbols:
        sources = {"undefined_symbols"}
        if symbol in defined_symbols:
            sources.add("defined_symbols")
            status = "project_defined"
            primitive_class = "native_project_symbol"
        elif _is_external_link_import(symbol):
            if symbol in runtime_symbols:
                sources.add("runtime_symbols")
            status = "external_link"
            primitive_class = _external_link_primitive_class(symbol)
        elif symbol in runtime_symbols:
            sources.add("runtime_symbols")
            if symbol in runtime_backed_symbols:
                status = "runtime_backed"
                primitive_class = _abi_primitive_class(symbol)
            elif _molt_runtime_namespace_symbol(symbol):
                status = "missing"
                primitive_class = "unknown_abi_symbol"
                errors.append(
                    f"object_closure runtime ABI symbol {symbol!r} is not in "
                    "the generated WASM ABI/link import surface"
                )
            else:
                status = "package_native"
                primitive_class = "native_package_symbol"
        elif symbol in runtime_backed_symbols:
            status = "missing"
            primitive_class = _abi_primitive_class(symbol)
            errors.append(
                f"object_closure undefined ABI symbol {symbol!r} is generated "
                "runtime-backed but missing from object_closure.runtime_symbols"
            )
        else:
            status = "missing"
            primitive_class = "unknown_abi_symbol"
            errors.append(
                f"object_closure undefined ABI symbol {symbol!r} is missing; "
                "declare it in object_closure.defined_symbols or in "
                "object_closure.runtime_symbols backed by the generated WASM ABI "
                "surface or package-native closure"
            )
        records.append(
            _ExternalNativeAbiSymbol(
                symbol=symbol,
                status=status,
                primitive_class=primitive_class,
                source="+".join(sorted(sources)),
            )
        )
    return tuple(records), errors


def _object_closure_c_api_symbol_board(
    manifest: Mapping[str, Any],
    *,
    external_symbols: Collection[str] | None = None,
) -> tuple[tuple[_ExternalNativeCapiSymbol, ...] | None, list[str]]:
    required_symbols = set(_manifest_object_closure_required_c_api_symbols(manifest))
    undefined_symbols = set(
        external_symbols
        if external_symbols is not None
        else _manifest_object_closure_undefined_symbols(manifest)
    )
    c_api_symbols = sorted(
        {
            symbol
            for symbol in (*required_symbols, *undefined_symbols)
            if is_c_api_symbol(symbol)
        }
    )
    if not c_api_symbols:
        return (), []

    molt_root = _molt_root_for_external_native_scan()
    surface, header_path, load_error = _load_c_api_scan_surface(
        molt_root,
        header_path=_c_api_scan_header_for_manifest(manifest, molt_root=molt_root),
    )
    if surface is None:
        return None, [
            "extension_manifest.json object_closure C-API symbol board could "
            f"not load {header_path}: {load_error}"
        ]

    defined_symbols = set(_manifest_object_closure_defined_symbols(manifest))
    runtime_symbols = set(_manifest_object_closure_runtime_symbols(manifest))
    records: list[_ExternalNativeCapiSymbol] = []
    errors: list[str] = []
    for symbol in c_api_symbols:
        if symbol in defined_symbols:
            status = "project_defined"
        else:
            status = surface.status_for(symbol)
        primitive_class = c_api_primitive_class(symbol)
        if symbol in runtime_symbols and symbol in undefined_symbols:
            if primitive_class == "numpy_c_api":
                if status in {"source_compile_only", "missing"}:
                    status = "package_native"
            elif status not in {"project_defined", "fail_fast"}:
                status = "cpython_abi_link"
        if (
            status == "missing"
            and primitive_class == "numpy_c_api"
            and symbol in required_symbols
            and symbol not in undefined_symbols
        ):
            status = "source_compile_only"
        sources: set[str] = set()
        if symbol in required_symbols:
            sources.add("required_c_api_symbols")
        if symbol in undefined_symbols:
            sources.add("undefined_symbols")
        if symbol in runtime_symbols:
            sources.add("runtime_symbols")
        if "undefined_symbols" in sources and status in {
            "source_compile_only",
            "fail_fast",
            "missing",
        }:
            errors.append(
                f"object_closure undefined C-API symbol {symbol!r} is {status}; "
                f"primitive_class={primitive_class}"
            )
        elif status in {"fail_fast", "missing"}:
            errors.append(
                f"object_closure required C-API symbol {symbol!r} is {status}; "
                f"primitive_class={primitive_class}"
            )
        records.append(
            _ExternalNativeCapiSymbol(
                symbol=symbol,
                status=status,
                primitive_class=primitive_class,
                source="+".join(sorted(sources)),
            )
        )
    return tuple(records), errors


def _validate_wasm_relocatable_undefined_symbol_custody(
    *,
    package: str,
    artifact_path: Path,
    manifest: Mapping[str, Any],
    artifact_kind: str,
    binary_import_symbols: Sequence[str] | None = None,
) -> list[str]:
    if artifact_kind != "wasm_relocatable_object":
        return []
    if binary_import_symbols is None:
        binary_import_symbols, import_errors = _wasm_relocatable_import_symbols(
            package=package,
            artifact_path=artifact_path,
            artifact_kind=artifact_kind,
            context="object_closure.undefined_symbols",
        )
        if import_errors:
            return import_errors
    assert binary_import_symbols is not None
    sidecar_symbols = _manifest_object_closure_undefined_symbols(manifest)
    external_sidecar_symbols = _manifest_object_closure_external_undefined_symbols(
        manifest
    )
    missing_from_sidecar = [
        symbol for symbol in binary_import_symbols if symbol not in sidecar_symbols
    ]
    stale_in_sidecar = [
        symbol
        for symbol in external_sidecar_symbols
        if symbol not in binary_import_symbols
    ]
    errors: list[str] = []
    if missing_from_sidecar:
        errors.append(
            f"{package}: {artifact_path.name} imports symbols absent from "
            "object_closure.undefined_symbols: " + ", ".join(missing_from_sidecar)
        )
    if stale_in_sidecar:
        errors.append(
            f"{package}: object_closure.undefined_symbols names symbols absent "
            f"from {artifact_path.name} imports: " + ", ".join(stale_in_sidecar)
        )
    return errors


def _validate_direct_symbol_callable_export_custody(
    *,
    package: str,
    artifact_path: Path,
    manifest: Mapping[str, Any],
    runtime_linkage: str,
    artifact_kind: str,
    callable_exports: Sequence[Any],
) -> list[str]:
    direct_symbols = tuple(
        sorted(
            {
                export.symbol
                for export in callable_exports
                if export.binding == "direct_symbol" and export.symbol
            }
        )
    )
    if not direct_symbols:
        return []
    if runtime_linkage != "static_link":
        return []
    if artifact_kind == "wasm_relocatable_object":
        try:
            exported_symbols = {
                export.name for export in read_wasm_function_exports(artifact_path)
            }
        except (OSError, UnicodeDecodeError, ValueError, IndexError) as exc:
            return [
                f"{package}: cannot validate direct_symbol callable exports for "
                f"{artifact_path.name}: {exc}"
            ]
        missing = [
            symbol for symbol in direct_symbols if symbol not in exported_symbols
        ]
        if missing:
            return [
                f"{package}: direct_symbol callable exports are absent from "
                f"{artifact_path.name} function exports: {', '.join(missing)}"
            ]
        return []
    if artifact_kind == "static_archive":
        defined_symbols = set(_manifest_object_closure_defined_symbols(manifest))
        if not defined_symbols:
            return [
                f"{package}: static_archive direct_symbol callable exports require "
                "object_closure.defined_symbols in extension_manifest.json"
            ]
        missing = [symbol for symbol in direct_symbols if symbol not in defined_symbols]
        if missing:
            return [
                f"{package}: direct_symbol callable exports are absent from "
                "object_closure.defined_symbols: " + ", ".join(missing)
            ]
        return []
    return []


def _validate_external_package_native_artifact(
    *,
    package: str,
    package_dir: Path,
    artifact_path: Path,
) -> tuple[_ExternalPackageNativeArtifact | None, list[str]]:
    errors: list[str] = []
    module_name = _external_extension_module_name(
        package=package,
        package_dir=package_dir,
        artifact_path=artifact_path,
    )
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return None, [
            f"{package}: native artifact {artifact_path} is missing "
            "extension_manifest.json sidecar"
        ]
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return None, [f"{package}: invalid extension manifest {manifest_path}: {exc}"]
    if not isinstance(manifest, dict):
        return None, [
            f"{package}: extension manifest {manifest_path} must be an object"
        ]
    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=manifest_path.parent,
        wheel_path=None,
        require_capabilities=True,
        required_abi=None,
        require_checksum=False,
        warn_missing_checksum=False,
    )
    errors.extend(f"{package}: {error}" for error in validation.errors)
    loader_kind = _required_manifest_str(manifest, "loader_kind", errors)
    if loader_kind and loader_kind != "libmolt_source":
        errors.append(
            f"{package}: external static package native artifacts must use "
            f"loader_kind 'libmolt_source', found {loader_kind!r}"
        )
    runtime_linkage = _required_manifest_str(manifest, "runtime_linkage", errors)
    artifact_kind = _required_manifest_str(manifest, "artifact_kind", errors)
    init_symbol = _required_manifest_str(manifest, "init_symbol", errors)
    manifest_module = _required_manifest_str(manifest, "module", errors)
    if manifest_module and manifest_module != module_name:
        errors.append(
            f"{package}: manifest module {manifest_module!r} does not match "
            f"native artifact module {module_name!r}"
        )
    manifest_extension = _required_manifest_str(manifest, "extension", errors)
    if manifest_extension and not _extension_path_matches_manifest(
        path=artifact_path,
        manifest_extension=manifest_extension,
        manifest_dir=manifest_path.parent,
        package_dir=package_dir,
    ):
        errors.append(
            f"{package}: manifest extension {manifest_extension!r} does not "
            f"match native artifact {artifact_path}"
        )
    expected_extension_sha = _required_manifest_str(
        manifest,
        "extension_sha256",
        errors,
    ).lower()
    actual_extension_sha = _sha256_file(artifact_path).lower()
    if expected_extension_sha and expected_extension_sha != actual_extension_sha:
        errors.append(
            f"{package}: extension_sha256 mismatch for {artifact_path.name}: "
            f"expected {expected_extension_sha}, got {actual_extension_sha}"
        )
    target_triple = _required_manifest_str(manifest, "target_triple", errors)
    platform_tag = _required_manifest_str(manifest, "platform_tag", errors)
    abi_tag = _required_manifest_str(manifest, "abi_tag", errors)
    provided_capsules = _manifest_str_tuple(manifest, "provided_capsules")
    required_capsules = _manifest_object_closure_required_capsules(manifest)
    errors.extend(
        f"{package}: {error}"
        for error in _validate_manifest_source_capsule_requirements(
            manifest,
            manifest_path=manifest_path,
        )
    )
    python_exports = _manifest_dotted_name_tuple(
        manifest,
        "python_exports",
        package=package,
        errors=errors,
    )
    callable_exports = _manifest_callable_exports(
        manifest,
        package=package,
        errors=errors,
    )
    package_source_root = _external_package_source_root(package_dir, package)
    manifest_support_file_sha256 = _manifest_support_file_sha256(
        manifest,
        manifest_path=manifest_path,
        package_source_root=package_source_root,
        errors=errors,
    )
    errors.extend(
        _validate_module_attr_callable_export_custody(
            package=package,
            manifest=manifest,
            manifest_path=manifest_path,
            module_name=module_name,
            callable_exports=callable_exports,
            support_file_sha256=manifest_support_file_sha256,
        )
    )
    wasm_import_symbols, wasm_import_errors = _wasm_relocatable_import_symbols(
        package=package,
        artifact_path=artifact_path,
        artifact_kind=artifact_kind,
        context="native artifact WASM imports",
    )
    errors.extend(wasm_import_errors)
    abi_symbols, abi_symbol_errors = _object_closure_abi_symbol_board(
        manifest,
        external_symbols=wasm_import_symbols,
    )
    errors.extend(f"{package}: {error}" for error in abi_symbol_errors)
    c_api_symbols, c_api_symbol_errors = _object_closure_c_api_symbol_board(
        manifest,
        external_symbols=wasm_import_symbols,
    )
    errors.extend(f"{package}: {error}" for error in c_api_symbol_errors)
    errors.extend(
        _validate_wasm_relocatable_undefined_symbol_custody(
            package=package,
            artifact_path=artifact_path,
            manifest=manifest,
            artifact_kind=artifact_kind,
            binary_import_symbols=wasm_import_symbols,
        )
    )
    errors.extend(
        _validate_direct_symbol_callable_export_custody(
            package=package,
            artifact_path=artifact_path,
            manifest=manifest,
            runtime_linkage=runtime_linkage,
            artifact_kind=artifact_kind,
            callable_exports=callable_exports,
        )
    )
    if errors:
        return None, errors
    assert c_api_symbols is not None
    package_init_support_file_sha256 = _external_native_support_file_sha256(
        package=package,
        package_dir=package_dir,
        module=module_name,
    )
    support_file_sha256 = tuple(
        sorted({*package_init_support_file_sha256, *manifest_support_file_sha256})
    )
    return (
        _ExternalPackageNativeArtifact(
            package=package,
            module=module_name,
            package_dir=package_dir.resolve(),
            path=artifact_path.resolve(),
            manifest_path=manifest_path.resolve(),
            extension_sha256=actual_extension_sha,
            manifest_sha256=_sha256_file(manifest_path),
            capabilities=tuple(validation.capabilities),
            abi_tag=abi_tag,
            target_triple=target_triple,
            platform_tag=platform_tag,
            init_symbol=init_symbol,
            runtime_linkage=runtime_linkage,
            artifact_kind=artifact_kind,
            support_file_sha256=support_file_sha256,
            provided_capsules=provided_capsules,
            required_capsules=required_capsules,
            python_exports=python_exports,
            callable_exports=callable_exports,
            abi_symbols=abi_symbols,
            c_api_symbols=c_api_symbols,
        ),
        [],
    )


def _peek_external_artifact_provided_capsules(
    *,
    artifact_path: Path,
    package_dir: Path,
) -> tuple[str, ...]:
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return ()
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(manifest, dict):
        return ()
    return _manifest_str_tuple(manifest, "provided_capsules")


def _peek_external_artifact_python_exports(
    *,
    package: str,
    artifact_path: Path,
    package_dir: Path,
) -> tuple[str, ...]:
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return ()
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(manifest, dict):
        return ()
    errors: list[str] = []
    return _manifest_dotted_name_tuple(
        manifest,
        "python_exports",
        package=package,
        errors=errors,
    )


def _peek_external_artifact_callable_export_names(
    *,
    package: str,
    artifact_path: Path,
    package_dir: Path,
) -> tuple[str, ...]:
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return ()
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(manifest, dict):
        return ()
    errors: list[str] = []
    return tuple(
        export.qualified_name
        for export in _manifest_callable_exports(
            manifest,
            package=package,
            errors=errors,
        )
    )


def _capsule_provider_errors(
    artifacts: Sequence[_ExternalPackageNativeArtifact],
) -> list[str]:
    providers: dict[str, str] = {}
    errors: list[str] = []
    for artifact in artifacts:
        for capsule in artifact.provided_capsules:
            existing = providers.get(capsule)
            if existing is not None and existing != artifact.module:
                errors.append(
                    f"{artifact.package}: capsule {capsule!r} is provided by "
                    f"multiple native artifacts: {existing}, {artifact.module}"
                )
                continue
            providers[capsule] = artifact.module
    return errors


def _missing_capsule_requirements(
    artifacts: Sequence[_ExternalPackageNativeArtifact],
) -> dict[str, tuple[_ExternalPackageNativeArtifact, ...]]:
    provided = {
        capsule for artifact in artifacts for capsule in artifact.provided_capsules
    }
    missing: dict[str, list[_ExternalPackageNativeArtifact]] = {}
    for artifact in artifacts:
        for capsule in artifact.required_capsules:
            if capsule not in provided:
                missing.setdefault(capsule, []).append(artifact)
    return {
        capsule: tuple(sorted(consumers, key=lambda artifact: artifact.module))
        for capsule, consumers in sorted(missing.items())
    }


def _close_external_native_capsule_provider_artifacts(
    artifacts: Sequence[_ExternalPackageNativeArtifact],
    provider_candidates: Sequence[tuple[str, Path, Path, str]],
) -> tuple[tuple[_ExternalPackageNativeArtifact, ...] | None, list[str]]:
    selected = list(artifacts)
    selected_keys = {(artifact.package, artifact.path) for artifact in selected}
    remaining = {
        (package, artifact_path): (
            package,
            package_dir,
            artifact_path,
            module_name,
        )
        for package, package_dir, artifact_path, module_name in provider_candidates
        if (package, artifact_path) not in selected_keys
    }
    while True:
        provider_errors = _capsule_provider_errors(selected)
        if provider_errors:
            return None, provider_errors
        missing = _missing_capsule_requirements(selected)
        if not missing:
            return tuple(selected), []
        to_add: dict[
            tuple[str, Path],
            tuple[str, Path, Path, str, _ExternalPackageNativeArtifact],
        ] = {}
        errors: list[str] = []
        for capsule, consumers in missing.items():
            consumer_modules = ", ".join(consumer.module for consumer in consumers)
            consumer_keys = {
                (consumer.runtime_linkage, consumer.target_triple.lower())
                for consumer in consumers
            }
            providers: list[
                tuple[
                    str,
                    Path,
                    Path,
                    str,
                    _ExternalPackageNativeArtifact,
                ]
            ] = []
            incompatible_providers: list[str] = []
            for candidate in remaining.values():
                package, package_dir, artifact_path, _module_name = candidate
                if capsule not in _peek_external_artifact_provided_capsules(
                    artifact_path=artifact_path,
                    package_dir=package_dir,
                ):
                    continue
                provider, provider_errors = _validate_external_package_native_artifact(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                if provider_errors:
                    return None, provider_errors
                assert provider is not None
                provider_key = (
                    provider.runtime_linkage,
                    provider.target_triple.lower(),
                )
                if provider_key in consumer_keys:
                    providers.append((*candidate, provider))
                else:
                    incompatible_providers.append(
                        f"{provider.module}={provider.runtime_linkage}/"
                        f"{provider.target_triple}"
                    )
            if not providers:
                suffix = ""
                if incompatible_providers:
                    suffix = "; incompatible provider artifact(s): " + ", ".join(
                        sorted(incompatible_providers)
                    )
                consumer_key_text = ", ".join(
                    f"{linkage}/{target}" for linkage, target in sorted(consumer_keys)
                )
                errors.append(
                    "native capsule "
                    f"{capsule!r} required by {consumer_modules} has no "
                    "target-compatible validated provider artifact in the admitted "
                    f"package plan for {consumer_key_text}{suffix}"
                )
                continue
            if len(providers) > 1:
                provider_names = ", ".join(sorted(item[3] for item in providers))
                errors.append(
                    "native capsule "
                    f"{capsule!r} required by {consumer_modules} has multiple "
                    f"candidate provider artifacts: {provider_names}"
                )
                continue
            provider = providers[0]
            to_add[(provider[0], provider[2])] = provider
        if errors:
            return None, errors
        if not to_add:
            return None, [
                "native capsule provider closure made no progress for "
                + ", ".join(missing)
            ]
        for package, _package_dir, artifact_path, _module_name, artifact in sorted(
            to_add.values(),
            key=lambda item: (item[0], item[3], str(item[2])),
        ):
            selected.append(artifact)
            key = (artifact.package, artifact.path)
            selected_keys.add(key)
            remaining.pop(key, None)


def _resolve_external_package_native_artifact_plan(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
    required_modules: Collection[str] | None = None,
) -> tuple[_ExternalPackageNativeArtifactPlan | None, list[str]]:
    artifacts: list[_ExternalPackageNativeArtifact] = []
    errors: list[str] = []
    errors.extend(
        _external_package_native_artifact_candidate_errors(
            external_module_roots=external_module_roots,
            admitted_packages=admitted_packages,
        )
    )
    if errors:
        return None, errors
    required = frozenset(required_modules) if required_modules is not None else None
    provider_candidates: list[tuple[str, Path, Path, str]] = []
    required_package_roots = (
        {
            package
            for package in admitted_packages
            if required is not None
            and _source_recompiled_external_package_root(package) is not None
            and package in required
        }
        if required is not None
        else set()
    )
    package_root_providers: dict[str, set[str]] = {
        package: set() for package in required_package_roots
    }
    selected_modules: set[tuple[str, str]] = set()
    for package in sorted(admitted_packages):
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            for artifact_path in _iter_external_package_native_artifacts(package_dir):
                module_name = _external_extension_module_name(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                provider_candidates.append(
                    (package, package_dir, artifact_path, module_name)
                )
                python_exports = _peek_external_artifact_python_exports(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                callable_exports = _peek_external_artifact_callable_export_names(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                provider_names = (
                    module_name,
                    *python_exports,
                    *callable_exports,
                )
                module_key = (package, module_name)
                if module_key in selected_modules:
                    continue
                if package in package_root_providers:
                    package_root_providers[package].update(provider_names)
                if (
                    required is not None
                    and not _external_native_artifact_module_required(
                        package=package,
                        module_name=module_name,
                        required_modules=required,
                    )
                    and not required.intersection(python_exports)
                    and not required.intersection(callable_exports)
                ):
                    continue
                artifact, artifact_errors = _validate_external_package_native_artifact(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                errors.extend(artifact_errors)
                if artifact is not None:
                    artifacts.append(artifact)
                    selected_modules.add(module_key)
    for package, providers in sorted(package_root_providers.items()):
        if any(
            _external_native_provider_reaches_required(
                provider_name=provider,
                required_name=package,
            )
            for provider in providers
        ):
            continue
        provider_summary = ", ".join(sorted(providers)) or "<none>"
        errors.append(
            f"{package}: required source-recompiled package import {package!r} "
            "has no manifest-symbol owner in admitted native artifacts. Child "
            "artifact modules are not package-root import custody; publish "
            f"{package!r} in python_exports before graph/backend admission. "
            "Candidate providers: "
            f"{provider_summary}"
        )
    if errors:
        return None, errors
    closed_artifacts, capsule_errors = (
        _close_external_native_capsule_provider_artifacts(
            artifacts,
            provider_candidates,
        )
    )
    if capsule_errors:
        return None, capsule_errors
    assert closed_artifacts is not None
    return (
        _ExternalPackageNativeArtifactPlan(
            artifacts=tuple(
                sorted(closed_artifacts, key=lambda item: (item.module, str(item.path)))
            )
        ),
        [],
    )


def _external_native_artifact_error_summary(
    errors: Sequence[str],
    *,
    limit: int = 12,
) -> str:
    if len(errors) <= limit:
        return "; ".join(errors)
    shown = "; ".join(errors[:limit])
    remaining = len(errors) - limit
    return (
        f"{shown}; ... and {remaining} more external native artifact custody error(s)."
    )


def _first_external_package_native_source_marker(package_dir: Path) -> Path | None:
    for current_root, dirnames, filenames in os.walk(package_dir):
        dirnames[:] = sorted(
            dirname
            for dirname in dirnames
            if dirname not in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_EXCLUDED_DIRS
            and not (Path(current_root) / dirname).is_symlink()
        )
        current = Path(current_root)
        for filename in sorted(filenames):
            path = current / filename
            if path.is_symlink():
                continue
            lowered = filename.lower()
            if _is_external_package_native_artifact(path) or any(
                lowered.endswith(suffix)
                for suffix in _EXTERNAL_PACKAGE_NATIVE_SOURCE_SUFFIXES
            ):
                return path.resolve()
    return None


def _manifest_is_wasm_static_link_artifact(manifest: Mapping[str, Any]) -> bool:
    if manifest.get("loader_kind") != "libmolt_source":
        return False
    if manifest.get("runtime_linkage") != "static_link":
        return False
    artifact_kind = manifest.get("artifact_kind")
    if artifact_kind not in {"wasm_relocatable_object", "static_archive"}:
        return False
    target_triple = manifest.get("target_triple")
    return isinstance(target_triple, str) and target_triple.lower().startswith("wasm32")


def _external_package_has_wasm_static_link_artifact(package_dir: Path) -> bool:
    for artifact_path in _iter_external_package_native_artifacts(package_dir):
        manifest_path = _find_external_extension_manifest(
            artifact_path=artifact_path,
            package_dir=package_dir,
        )
        if manifest_path is None:
            continue
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(manifest, Mapping) and _manifest_is_wasm_static_link_artifact(
            manifest
        ):
            return True
    return False


def _wasm_static_link_artifact_export_names(
    *,
    package: str,
    package_dir: Path,
    artifact_path: Path,
) -> tuple[str, ...]:
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return ()
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(manifest, Mapping):
        return ()
    if not _manifest_is_wasm_static_link_artifact(manifest):
        return ()
    errors: list[str] = []
    python_exports = _manifest_dotted_name_tuple(
        manifest,
        "python_exports",
        package=package,
        errors=errors,
    )
    callable_exports = _manifest_callable_exports(
        manifest,
        package=package,
        errors=errors,
    )
    return (
        *python_exports,
        *(export.qualified_name for export in callable_exports),
    )


def _wasm_external_package_export_custody_errors(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
) -> list[str]:
    errors: list[str] = []
    for package in sorted(admitted_packages):
        native_root = _source_recompiled_external_package_root(package)
        if native_root is None:
            continue
        package_dirs: list[Path] = []
        static_artifacts: list[Path] = []
        exported_names: set[str] = set()
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            package_dirs.append(package_dir)
            for artifact_path in _iter_external_package_native_artifacts(package_dir):
                manifest_path = _find_external_extension_manifest(
                    artifact_path=artifact_path,
                    package_dir=package_dir,
                )
                if manifest_path is None:
                    continue
                try:
                    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    continue
                if not isinstance(manifest, Mapping):
                    continue
                if not _manifest_is_wasm_static_link_artifact(manifest):
                    continue
                static_artifacts.append(artifact_path.resolve())
                exported_names.update(
                    _wasm_static_link_artifact_export_names(
                        package=package,
                        package_dir=package_dir,
                        artifact_path=artifact_path,
                    )
                )
        if not static_artifacts or exported_names:
            continue
        roots = ", ".join(str(path) for path in package_dirs)
        artifacts = ", ".join(str(path) for path in sorted(static_artifacts))
        errors.append(
            f"{package}: admitted WASM source-recompiled package root "
            f"{native_root!r} has static_link libmolt_source artifacts but no "
            "manifest-declared python_exports or callable_exports. Import "
            f"visibility for {package!r} must not select native artifacts by "
            "directory ancestry; publish package-symbol export custody in "
            "extension_manifest.json so reachable object closure can stay "
            f"tree-shaken. Artifacts: {artifacts}. Roots: {roots}"
        )
    return errors


def _wasm_external_package_native_source_errors(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
) -> list[str]:
    errors: list[str] = []
    for package in sorted(admitted_packages):
        package_dirs: list[Path] = []
        first_marker: Path | None = None
        has_wasm_static_link_artifact = False
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            package_dirs.append(package_dir)
            marker = _first_external_package_native_source_marker(package_dir)
            if marker is not None and first_marker is None:
                first_marker = marker
            if _external_package_has_wasm_static_link_artifact(package_dir):
                has_wasm_static_link_artifact = True
        if first_marker is None or has_wasm_static_link_artifact:
            continue
        roots = ", ".join(str(path) for path in package_dirs)
        errors.append(
            f"{package}: admitted WASM external static package contains "
            f"native source/artifact marker {first_marker} but has no wasm32 "
            "static_link libmolt_source artifact manifest in its admitted "
            "package roots. Source roots alone are not linkable for WASM; "
            "publish source-recompiled native artifacts with extension_manifest.json "
            f"and python_exports for package symbols before admission. Roots: {roots}"
        )
    return errors


def _resolve_import_admission_policy(
    *,
    external_module_roots: Sequence[Path],
    json_output: bool,
    defer_native_artifacts: bool = False,
    target: str | None = None,
) -> tuple[_ImportAdmissionPolicy | None, _CliFailure | None]:
    packages, error = _parse_external_static_packages(
        os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES", "")
    )
    if error is not None:
        return None, _fail(error, json_output, command="build")
    candidate_errors = _external_package_native_artifact_candidate_errors(
        external_module_roots=external_module_roots,
        admitted_packages=packages,
    )
    if candidate_errors:
        return None, _fail(
            "External static package native-artifact custody errors: "
            + _external_native_artifact_error_summary(candidate_errors),
            json_output,
            command="build",
        )
    if target == "wasm":
        wasm_native_source_errors = _wasm_external_package_native_source_errors(
            external_module_roots=external_module_roots,
            admitted_packages=packages,
        )
        if wasm_native_source_errors:
            return None, _fail(
                "External static package native-artifact custody errors: "
                + _external_native_artifact_error_summary(wasm_native_source_errors),
                json_output,
                command="build",
            )
        wasm_export_custody_errors = _wasm_external_package_export_custody_errors(
            external_module_roots=external_module_roots,
            admitted_packages=packages,
        )
        if wasm_export_custody_errors:
            return None, _fail(
                "External static package native-artifact custody errors: "
                + _external_native_artifact_error_summary(wasm_export_custody_errors),
                json_output,
                command="build",
            )
    if defer_native_artifacts:
        native_plan = _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    else:
        native_plan, native_plan_errors = (
            _resolve_external_package_native_artifact_plan(
                external_module_roots=external_module_roots,
                admitted_packages=packages,
            )
        )
        if native_plan_errors:
            return None, _fail(
                "External static package native-artifact custody errors: "
                + _external_native_artifact_error_summary(native_plan_errors),
                json_output,
                command="build",
            )
        assert native_plan is not None
    return _ImportAdmissionPolicy(
        external_roots=tuple(external_module_roots),
        admitted_external_packages=packages,
        native_artifact_source_packages=(
            _native_artifact_source_packages_for_admission(packages)
        ),
        native_artifact_plan=native_plan,
    ), None


def _external_package_source_root(package_dir: Path, package: str) -> Path:
    resolved = package_dir.resolve()
    package_parts = tuple(part for part in package.split(".") if part)
    if (
        package_parts
        and len(resolved.parts) >= len(package_parts)
        and tuple(resolved.parts[-len(package_parts) :]) == package_parts
    ):
        return resolved.parents[len(package_parts) - 1]
    return resolved.parent


def _external_package_init_source_paths(
    *,
    package_dir: Path,
    package: str,
) -> tuple[Path, ...]:
    package_root = _external_package_source_root(package_dir, package)
    package_parts = tuple(part for part in package.split(".") if part)
    return tuple(
        package_root.joinpath(*package_parts[:index], "__init__.py")
        for index in range(1, len(package_parts) + 1)
    )


def _external_native_support_source_paths_for(
    *,
    package: str,
    package_dir: Path,
    module: str,
) -> tuple[Path, ...]:
    out: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            out.append(path)

    package_source_root = _external_package_source_root(package_dir, package)
    module_parent_parts = tuple(part for part in module.split(".")[:-1] if part)
    for index in range(1, len(module_parent_parts) + 1):
        init_path = package_source_root.joinpath(
            *module_parent_parts[:index],
            "__init__.py",
        )
        add(init_path)

    return tuple(out)


def _external_native_support_source_paths(
    artifact: _ExternalPackageNativeArtifact,
) -> tuple[Path, ...]:
    out: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            out.append(path)

    for path in _external_native_support_source_paths_for(
        package=artifact.package,
        package_dir=artifact.package_dir,
        module=artifact.module,
    ):
        add(path)
    package_source_root = _external_package_source_root(
        artifact.package_dir,
        artifact.package,
    )
    for rel_path, _sha256 in artifact.support_file_sha256:
        add(package_source_root / Path(rel_path))
    return tuple(out)


def _external_native_support_file_sha256(
    *,
    package: str,
    package_dir: Path,
    module: str,
) -> tuple[tuple[str, str], ...]:
    package_source_root = _external_package_source_root(package_dir, package)
    support: list[tuple[str, str]] = []
    for source_path in _external_native_support_source_paths_for(
        package=package,
        package_dir=package_dir,
        module=module,
    ):
        if not source_path.is_file():
            continue
        rel_path = (
            source_path.resolve().relative_to(package_source_root.resolve()).as_posix()
        )
        support.append((rel_path, _sha256_file(source_path).lower()))
    return tuple(sorted(support))


def _external_native_disallowed_shim_source_paths(
    artifact: _ExternalPackageNativeArtifact,
) -> tuple[Path, ...]:
    out: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            out.append(path)

    artifact_path = artifact.path
    add(artifact_path.with_name(f"{artifact_path.name}.molt.py"))
    add(artifact_path.with_name(f"{artifact_path.name}.py"))
    stripped = artifact_path.with_suffix("")
    if stripped != artifact_path:
        add(stripped.with_name(f"{stripped.name}.molt.py"))
        add(stripped.with_name(f"{stripped.name}.py"))
        for marker in (".cpython-", ".abi", ".cp"):
            marker_index = stripped.name.rfind(marker)
            if marker_index > 0:
                prefix_name = stripped.name[:marker_index]
                add(stripped.with_name(f"{prefix_name}.molt.py"))
                add(stripped.with_name(f"{prefix_name}.py"))
    if artifact_path.parent.name == "__pycache__":
        parent = artifact_path.parent.parent
        stem = artifact_path.name.rsplit(".", 1)[0]
        module_stem = stem.split(".", 1)[0]
        if module_stem:
            add(parent / f"{module_stem}.molt.py")
            add(parent / f"{module_stem}.py")
    local_name = artifact.module.rsplit(".", 1)[-1]
    if local_name:
        add(artifact_path.parent / f"{local_name}.molt.py")
        add(artifact_path.parent / f"{local_name}.py")
        add(artifact_path.parent / local_name / "__init__.molt.py")
        add(artifact_path.parent / local_name / "__init__.py")
    if artifact_path.name.startswith("__init__."):
        add(artifact_path.parent / "__init__.molt.py")
        add(artifact_path.parent / "__init__.py")
    return tuple(out)


def _external_staged_path_for_source(
    *,
    runtime_root: Path,
    package_source_root: Path,
    source_path: Path,
) -> Path:
    resolved_source = source_path.resolve()
    try:
        relative = resolved_source.relative_to(package_source_root)
    except ValueError as exc:
        raise OSError(
            f"external native support path escapes admitted package root: {source_path}"
        ) from exc
    return runtime_root / relative


def _remove_staged_external_candidate(path: Path) -> None:
    with contextlib.suppress(OSError):
        if path.exists() or path.is_symlink():
            _remove_file_or_tree(path)


def _stage_external_native_required_file(
    *,
    source_path: Path,
    staged_path: Path,
    expected_sha256: str,
    label: str,
) -> None:
    expected = expected_sha256.lower()
    actual = _sha256_file(source_path).lower()
    if actual != expected:
        raise OSError(
            f"External native artifact {label} checksum changed before staging: "
            f"{source_path} expected {expected}, got {actual}"
        )
    _atomic_copy_file(source_path, staged_path)
    staged = _sha256_file(staged_path).lower()
    if staged != expected:
        _remove_staged_external_candidate(staged_path)
        raise OSError(
            f"External native artifact {label} changed during staging: "
            f"{source_path} expected {expected}, staged {staged}"
        )


def _stage_external_native_support_files(
    artifact: _ExternalPackageNativeArtifact,
    *,
    runtime_root: Path,
    package_source_root: Path,
) -> tuple[Path, ...]:
    staged_paths: list[Path] = []
    expected_support = dict(artifact.support_file_sha256)
    for source_path in _external_native_support_source_paths(artifact):
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=source_path,
        )
        rel_path = (
            source_path.resolve().relative_to(package_source_root.resolve()).as_posix()
        )
        if source_path.is_file():
            expected_sha256 = expected_support.get(rel_path)
            if expected_sha256 is None:
                raise OSError(
                    "External native support file appeared after plan resolution: "
                    f"{source_path}"
                )
            _stage_external_native_required_file(
                source_path=source_path,
                staged_path=staged_path,
                expected_sha256=expected_sha256,
                label=f"support file {rel_path}",
            )
            staged_paths.append(staged_path)
        elif rel_path in expected_support:
            raise OSError(
                "External native support file disappeared after plan resolution: "
                f"{source_path}"
            )
        else:
            _remove_staged_external_candidate(staged_path)
    for source_path in _external_native_disallowed_shim_source_paths(artifact):
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=source_path,
        )
        _remove_staged_external_candidate(staged_path)
    return tuple(staged_paths)


def _stage_external_package_native_artifacts_for_build(
    native_artifact_plan: _ExternalPackageNativeArtifactPlan,
    *,
    artifacts_root: Path,
) -> tuple[_StagedExternalPackageNativeArtifact, ...]:
    if not native_artifact_plan.artifacts:
        return ()
    runtime_root = (
        artifacts_root / "external_static_packages" / native_artifact_plan.digest()
    )
    staged_artifacts: list[_StagedExternalPackageNativeArtifact] = []
    for artifact in native_artifact_plan.artifacts:
        package_source_root = _external_package_source_root(
            artifact.package_dir,
            artifact.package,
        )
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=artifact.path,
        )
        staged_manifest_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=artifact.manifest_path,
        )
        _stage_external_native_required_file(
            source_path=artifact.path,
            staged_path=staged_path,
            expected_sha256=artifact.extension_sha256,
            label="extension",
        )
        _stage_external_native_required_file(
            source_path=artifact.manifest_path,
            staged_path=staged_manifest_path,
            expected_sha256=artifact.manifest_sha256,
            label="manifest",
        )
        staged_support_paths = _stage_external_native_support_files(
            artifact,
            runtime_root=runtime_root,
            package_source_root=package_source_root,
        )
        staged_artifacts.append(
            _StagedExternalPackageNativeArtifact(
                package=artifact.package,
                module=artifact.module,
                runtime_root=runtime_root,
                source_path=artifact.path,
                source_manifest_path=artifact.manifest_path,
                staged_path=staged_path,
                staged_manifest_path=staged_manifest_path,
                staged_support_paths=staged_support_paths,
                extension_sha256=artifact.extension_sha256,
                manifest_sha256=artifact.manifest_sha256,
                capabilities=artifact.capabilities,
                abi_tag=artifact.abi_tag,
                target_triple=artifact.target_triple,
                platform_tag=artifact.platform_tag,
                init_symbol=artifact.init_symbol,
                runtime_linkage=artifact.runtime_linkage,
                artifact_kind=artifact.artifact_kind,
                support_file_sha256=artifact.support_file_sha256,
                provided_capsules=artifact.provided_capsules,
                required_capsules=artifact.required_capsules,
                python_exports=artifact.python_exports,
                callable_exports=artifact.callable_exports,
                abi_symbols=artifact.abi_symbols,
                c_api_symbols=artifact.c_api_symbols,
            )
        )
    return tuple(staged_artifacts)


def _external_native_artifact_output_custody_error(
    *,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan,
    output_layout: _BuildOutputLayout,
    target: str,
) -> str | None:
    if not native_artifact_plan.artifacts:
        return None
    if (
        not output_layout.is_wasm
        and not output_layout.is_rust_transpile
        and not output_layout.is_luau_transpile
        and not output_layout.is_mlir_emit
        and output_layout.emit_mode == "bin"
    ):
        expected_target = (output_layout.target_triple or _host_target_triple()).lower()
        mismatches = [
            f"{artifact.module}={artifact.target_triple}"
            for artifact in native_artifact_plan.artifacts
            if artifact.target_triple.lower() != expected_target
        ]
        if mismatches:
            return (
                "External static package native-artifact target mismatch: "
                f"expected {expected_target}; " + ", ".join(mismatches)
            )
        linkage_mismatches = [
            f"{artifact.module}={artifact.runtime_linkage}/{artifact.artifact_kind}"
            for artifact in native_artifact_plan.artifacts
            if artifact.runtime_linkage != "host_resolved"
            or artifact.artifact_kind != "shared_library"
        ]
        if linkage_mismatches:
            return (
                "External static package native binary output requires "
                "host_resolved shared_library artifacts: "
                + ", ".join(linkage_mismatches)
            )
        return None
    if output_layout.is_wasm and output_layout.linked:
        mismatches = [
            f"{artifact.module}={artifact.runtime_linkage}/"
            f"{artifact.artifact_kind}/{artifact.target_triple}"
            for artifact in native_artifact_plan.artifacts
            if artifact.runtime_linkage != "static_link"
            or artifact.artifact_kind
            not in {
                "wasm_relocatable_object",
                "static_archive",
            }
            or not artifact.target_triple.lower().startswith("wasm32")
        ]
        if mismatches:
            return (
                "Linked WASM external static packages require wasm32 static_link "
                "libmolt_source artifacts: " + ", ".join(mismatches)
            )
        return None
    packages = ", ".join(
        sorted({artifact.package for artifact in native_artifact_plan.artifacts})
    )
    return (
        "External static packages require native binary output with host_resolved "
        "shared artifacts, or linked WASM output with wasm32 static_link artifacts. "
        f"Unsupported target/emit combination: target={target}, "
        f"emit={output_layout.emit_mode}, packages={packages}."
    )
