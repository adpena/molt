from __future__ import annotations

import os
from pathlib import Path

from molt.c_api_symbols import c_api_primitive_class, is_c_api_external_requirement
from molt.cli.config_resolution import _config_value
from molt.cli.deps import _load_toml
from molt.cli.extension_manifest import _coerce_str_list
from molt.cli.extension_scan_surface import _extract_c_api_tokens
from molt.cli.extension_scan_surface import _extract_file_local_c_api_symbols
from molt.cli.extension_scan_surface import _extract_project_generated_c_api_prefixes
from molt.cli.extension_scan_surface import _extract_project_defined_c_api_symbols
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.extension_scan_surface import _matches_project_generated_c_api_prefix
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import (
    _find_molt_root,
    _find_project_root,
    _require_molt_root,
)


_EXTENSION_SCAN_SOURCE_SUFFIXES = {
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".pxd",
    ".pxi",
    ".pyx",
}
_EXTENSION_SCAN_EXCLUDED_DIRS = {
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


def _iter_extension_scan_dir_sources(
    root: Path, *, exclude_dirs: set[str] | None = None
) -> list[Path]:
    excluded_dirs = _EXTENSION_SCAN_EXCLUDED_DIRS | (exclude_dirs or set())
    source_paths: list[Path] = []
    for current_root, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(
            dirname
            for dirname in dirnames
            if dirname not in excluded_dirs
            and not (Path(current_root) / dirname).is_symlink()
        )
        current = Path(current_root)
        for filename in sorted(filenames):
            path = current / filename
            if (
                path.is_symlink()
                or path.suffix.lower() not in _EXTENSION_SCAN_SOURCE_SUFFIXES
            ):
                continue
            source_paths.append(path.resolve())
    return source_paths


def _resolve_extension_scan_sources(
    project_root: Path,
    explicit_sources: list[str] | None,
    *,
    exclude_dirs: set[str] | None = None,
) -> tuple[list[Path], list[str]]:
    errors: list[str] = []
    source_entries: list[str] = []
    if explicit_sources:
        source_entries = [
            entry for entry in explicit_sources if entry and entry.strip()
        ]
        if not source_entries:
            errors.append("--source must include at least one non-empty path")
    else:
        pyproject = _load_toml(project_root / "pyproject.toml")
        extension_meta = _config_value(pyproject, ["tool", "molt", "extension"])
        if not isinstance(extension_meta, dict):
            errors.append("pyproject.toml must contain [tool.molt.extension]")
        else:
            source_entries = _coerce_str_list(
                extension_meta.get("sources"),
                "tool.molt.extension.sources",
                errors,
                allow_empty=False,
            )
            if not source_entries:
                errors.append(
                    "tool.molt.extension.sources must include at least one source"
                )
    source_paths: list[Path] = []
    for entry in source_entries:
        source_path = Path(entry).expanduser()
        if not source_path.is_absolute():
            source_path = (project_root / source_path).absolute()
        if not source_path.exists():
            errors.append(f"source path not found: {source_path}")
            continue
        if source_path.is_dir():
            expanded = _iter_extension_scan_dir_sources(
                source_path,
                exclude_dirs=exclude_dirs,
            )
            if not expanded:
                suffixes = ", ".join(sorted(_EXTENSION_SCAN_SOURCE_SUFFIXES))
                errors.append(
                    f"source directory has no scannable extension sources "
                    f"({suffixes}): {source_path}"
                )
            source_paths.extend(expanded)
            continue
        if not source_path.is_file():
            errors.append(f"source path is not a regular file: {source_path}")
            continue
        if source_path.suffix.lower() not in _EXTENSION_SCAN_SOURCE_SUFFIXES:
            suffixes = ", ".join(sorted(_EXTENSION_SCAN_SOURCE_SUFFIXES))
            errors.append(
                f"source file has unsupported extension (expected one of "
                f"{suffixes}): {source_path}"
            )
            continue
        source_paths.append(source_path.resolve())
    deduped = sorted(set(source_paths), key=lambda path: path.as_posix())
    return deduped, errors


def extension_scan(
    project: str | None = None,
    sources: list[str] | None = None,
    exclude_dirs: list[str] | None = None,
    fail_on_missing: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    project_root = Path(project).expanduser() if project else Path.cwd()
    if not project_root.is_absolute():
        project_root = (Path.cwd() / project_root).absolute()
    if not project_root.exists() or not project_root.is_dir():
        return _fail(
            f"Project directory not found: {project_root}",
            json_output,
            command="extension-scan",
        )

    excluded_dir_names = {
        entry.strip()
        for entry in (exclude_dirs or [])
        if entry is not None and entry.strip()
    }
    source_paths, errors = _resolve_extension_scan_sources(
        project_root,
        sources,
        exclude_dirs=excluded_dir_names,
    )
    if errors:
        return _fail(
            "Extension scan configuration errors: " + "; ".join(errors),
            json_output,
            command="extension-scan",
        )

    cwd_root = _find_project_root(Path.cwd())
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "extension-scan")
    if root_error is not None:
        return root_error

    scan_surface, header_path, header_error = _load_c_api_scan_surface(molt_root)
    if header_error is not None:
        return _fail(
            f"Failed to read libmolt Python.h surface ({header_path}): {header_error}",
            json_output,
            command="extension-scan",
        )
    assert scan_surface is not None

    required_by_file: dict[str, list[str]] = {}
    missing_by_file: dict[str, list[str]] = {}
    fail_fast_by_file: dict[str, list[str]] = {}
    symbol_status_by_file: dict[str, dict[str, str]] = {}
    required_symbols: set[str] = set()
    project_defined_symbols: set[str] = set()
    project_generated_c_api_prefixes: set[str] = set()
    source_text_by_path: dict[Path, str] = {}
    file_local_symbols_by_path: dict[Path, set[str]] = {}
    for source_path in source_paths:
        try:
            source_text = source_path.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeError) as exc:
            return _fail(
                f"Failed to read source file {source_path}: {exc}",
                json_output,
                command="extension-scan",
            )
        source_text_by_path[source_path] = source_text
        project_defined_symbols.update(
            _extract_project_defined_c_api_symbols(source_text)
        )
        project_generated_c_api_prefixes.update(
            _extract_project_generated_c_api_prefixes(source_text)
        )
        file_local_symbols_by_path[source_path] = _extract_file_local_c_api_symbols(
            source_text
        )
    generated_prefixes = frozenset(project_generated_c_api_prefixes)

    def symbol_status(symbol: str) -> str:
        surface_status = scan_surface.status_for(symbol)
        if surface_status == "missing" and symbol in project_defined_symbols:
            return "project_defined"
        if surface_status == "missing" and _matches_project_generated_c_api_prefix(
            symbol,
            generated_prefixes,
        ):
            return "project_generated"
        return surface_status

    for source_path, source_text in source_text_by_path.items():
        file_required = sorted(
            {
                symbol
                for symbol in _extract_c_api_tokens(source_text)
                if is_c_api_external_requirement(symbol)
            }
            - file_local_symbols_by_path[source_path]
        )
        required_by_file[str(source_path)] = file_required
        required_symbols.update(file_required)
        file_missing = sorted(
            symbol for symbol in file_required if symbol_status(symbol) == "missing"
        )
        if file_missing:
            missing_by_file[str(source_path)] = file_missing
        file_fail_fast = sorted(
            symbol for symbol in file_required if symbol_status(symbol) == "fail_fast"
        )
        if file_fail_fast:
            fail_fast_by_file[str(source_path)] = file_fail_fast
        symbol_status_by_file[str(source_path)] = {
            symbol: symbol_status(symbol) for symbol in file_required
        }

    required_sorted = sorted(required_symbols)
    missing_sorted = sorted(
        symbol for symbol in required_sorted if symbol_status(symbol) == "missing"
    )
    fail_fast_sorted = sorted(
        symbol for symbol in required_sorted if symbol_status(symbol) == "fail_fast"
    )
    runtime_backed_used_sorted = sorted(
        symbol
        for symbol in required_sorted
        if symbol_status(symbol) == "runtime_backed"
    )
    source_compile_only_used_sorted = sorted(
        symbol
        for symbol in required_sorted
        if symbol_status(symbol) == "source_compile_only"
    )
    project_defined_used_sorted = sorted(
        symbol
        for symbol in required_sorted
        if symbol_status(symbol) == "project_defined"
    )
    project_generated_used_sorted = sorted(
        symbol
        for symbol in required_sorted
        if symbol_status(symbol) == "project_generated"
    )
    project_generated_used_sorted = sorted(
        symbol
        for symbol in required_sorted
        if symbol_status(symbol) == "project_generated"
    )
    supported_used_sorted = sorted(
        runtime_backed_used_sorted
        + source_compile_only_used_sorted
        + project_defined_used_sorted
        + project_generated_used_sorted
    )
    symbol_status_map = {symbol: symbol_status(symbol) for symbol in required_sorted}
    symbol_primitive_class = {
        symbol: c_api_primitive_class(symbol) for symbol in required_sorted
    }
    symbols_by_primitive_class: dict[str, list[str]] = {}
    for symbol in required_sorted:
        primitive_class = symbol_primitive_class[symbol]
        symbols_by_primitive_class.setdefault(primitive_class, []).append(symbol)
    symbols_by_primitive_class = {
        primitive_class: sorted(symbols)
        for primitive_class, symbols in sorted(symbols_by_primitive_class.items())
    }
    primitive_class_counts = {
        primitive_class: len(symbols)
        for primitive_class, symbols in symbols_by_primitive_class.items()
    }
    primitive_class_by_file = {
        file_path: {symbol: c_api_primitive_class(symbol) for symbol in symbols}
        for file_path, symbols in required_by_file.items()
    }
    warnings: list[str] = []
    if missing_sorted and not fail_on_missing:
        warnings.append(
            "Unsupported C/API symbols detected (run with --fail-on-missing to gate)."
        )
    if fail_fast_sorted and not fail_on_missing:
        warnings.append(
            "Fail-fast C/API symbols detected (run with --fail-on-missing to gate)."
        )
    status = "ok"
    if fail_on_missing and (missing_sorted or fail_fast_sorted):
        status = "error"

    if json_output:
        payload = _json_payload(
            "extension-scan",
            status,
            data={
                "project": str(project_root),
                "header": str(header_path),
                "source_count": len(source_paths),
                "required_symbol_count": len(required_sorted),
                "supported_symbol_count": len(supported_used_sorted),
                "missing_symbol_count": len(missing_sorted),
                "fail_fast_symbol_count": len(fail_fast_sorted),
                "runtime_backed_symbol_count": len(runtime_backed_used_sorted),
                "source_compile_only_symbol_count": len(
                    source_compile_only_used_sorted
                ),
                "project_defined_symbol_count": len(project_defined_used_sorted),
                "project_generated_symbol_count": len(project_generated_used_sorted),
                "exclude_dirs": sorted(excluded_dir_names),
                "required_symbols": required_sorted,
                "supported_symbols": supported_used_sorted,
                "runtime_backed_symbols": runtime_backed_used_sorted,
                "source_compile_only_symbols": source_compile_only_used_sorted,
                "project_defined_symbols": project_defined_used_sorted,
                "project_generated_symbols": project_generated_used_sorted,
                "project_generated_c_api_prefixes": sorted(generated_prefixes),
                "fail_fast_symbols": fail_fast_sorted,
                "missing_symbols": missing_sorted,
                "symbol_status": symbol_status_map,
                "symbol_primitive_class": symbol_primitive_class,
                "symbols_by_primitive_class": symbols_by_primitive_class,
                "primitive_class_counts": primitive_class_counts,
                "required_by_file": required_by_file,
                "missing_by_file": missing_by_file,
                "fail_fast_by_file": fail_fast_by_file,
                "symbol_status_by_file": symbol_status_by_file,
                "primitive_class_by_file": primitive_class_by_file,
                "fail_on_missing": fail_on_missing,
            },
            warnings=warnings,
            errors=["unsupported C-API symbols found"] if status == "error" else None,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Extension C-API scan header: {header_path}")
        print(f"Scanned source files: {len(source_paths)}")
        print(f"Required C/API symbols: {len(required_sorted)}")
        print(f"Supported C/API symbols used: {len(supported_used_sorted)}")
        print(f"Runtime-backed C/API symbols used: {len(runtime_backed_used_sorted)}")
        print(
            "Source-compile-only C/API symbols used: "
            f"{len(source_compile_only_used_sorted)}"
        )
        print(f"Project-defined C/API symbols used: {len(project_defined_used_sorted)}")
        print(
            "Project-generated C/API symbols used: "
            f"{len(project_generated_used_sorted)}"
        )
        print(f"Fail-fast C/API symbols: {len(fail_fast_sorted)}")
        print(f"Missing C/API symbols: {len(missing_sorted)}")
        if primitive_class_counts:
            print("C/API primitive classes:")
            for primitive_class, count in primitive_class_counts.items():
                print(f"  {primitive_class}: {count}")
        if fail_fast_sorted:
            limit = len(fail_fast_sorted) if verbose else min(30, len(fail_fast_sorted))
            for symbol in fail_fast_sorted[:limit]:
                print(f"FAIL_FAST: {symbol}")
            if limit < len(fail_fast_sorted):
                print(
                    f"... {len(fail_fast_sorted) - limit} "
                    "additional fail-fast symbols omitted"
                )
        if missing_sorted:
            limit = len(missing_sorted) if verbose else min(30, len(missing_sorted))
            for symbol in missing_sorted[:limit]:
                print(f"MISSING: {symbol}")
            if limit < len(missing_sorted):
                print(f"... {len(missing_sorted) - limit} additional symbols omitted")
        if verbose:
            for file_path in sorted(fail_fast_by_file):
                print(
                    f"{file_path} fail-fast: {', '.join(fail_fast_by_file[file_path])}"
                )
            for file_path in sorted(missing_by_file):
                print(f"{file_path} missing: {', '.join(missing_by_file[file_path])}")
        for warning in warnings:
            print(f"WARN: {warning}")

    if status == "error":
        return 1
    return 0
