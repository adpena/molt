from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, TypeVar

from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES,
)
from molt._wasm_runtime_exports import wasm_static_link_runtime_symbols_for_imports
from molt.c_api_symbols import (
    is_c_api_external_requirement,
    is_cpython_abi_dynamic_import_symbol,
)
from molt.cli import source_extension_cython as _source_extension_cython
from molt.python_module_names import encode_python_module_names
from molt.cli.compiler_target import (
    compiler_target_triple,
    validate_compiler_target,
    source_extension_compiler_dialect,
    compiler_argument_spans,
    COMPILER_OWNED_OPTIONS,
)
from molt.cli.source_extension_target import (
    source_extension_link_dialect,
    source_extension_target_is_wasm,
)
from molt.cli.source_extension_link_arguments import source_extension_link_arguments
from molt.cli.source_extension_python_provider import SourceExtensionPythonProvider
from molt.cli.source_extension_language import (
    SourceExtensionLanguage,
    resolve_source_extension_compile_language,
)
from molt.cli.source_extension_input_custody import (
    SourceExtensionInputCustodyError,
    read_source_extension_manifest_input,
    source_extension_manifest_input_rows,
)
from molt.cli.source_extension_manifest_codec import (
    _expand_source_extension_manifest_authorities,
    _manifest_dependencies,
)
from molt.cli.source_extension_runtime_imports import (
    source_extension_runtime_python_imports,
)
from molt.cli.extension_scan_surface import _extract_project_generated_c_api_prefixes
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.extension_scan_surface import _matches_project_generated_c_api_prefix
from molt.cli.extension_scan_surface import _strip_c_like_comments_and_literals
from molt.cli.external_link_providers import (
    wasm_external_link_provider_symbol_classes,
)
from molt.file_hashing import _sha256_file
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.cli.source_extension_object_closure import (
    SourceExtensionObjectClosureError,
    source_extension_wasm_import_receipts,
    validate_source_extension_object_closure,
    validate_source_extension_wasm_import_shapes,
)
from molt.wasm_artifact import WasmImport, WasmRelocatableObjectInterface


_MOLT_NUMPY_ARRAY_API_CAPSULE = "numpy.core._multiarray_umath._ARRAY_API"
_SourceScanResult = TypeVar("_SourceScanResult")
_MOLT_NUMPY_UFUNC_API_CAPSULE = "numpy.core._multiarray_umath._UFUNC_API"
_C_IDENTIFIER_RE = re.compile(r"\b[A-Za-z_][A-Za-z0-9_]*\b")
_SOURCE_EXTENSION_CAPSULE_IMPORT_TOKENS: dict[str, str] = {
    "import_array": _MOLT_NUMPY_ARRAY_API_CAPSULE,
    "import_array1": _MOLT_NUMPY_ARRAY_API_CAPSULE,
    "import_array2": _MOLT_NUMPY_ARRAY_API_CAPSULE,
    "_import_array": _MOLT_NUMPY_ARRAY_API_CAPSULE,
    "PyArray_ImportNumPyAPI": _MOLT_NUMPY_ARRAY_API_CAPSULE,
    "PyUFunc_ImportUFuncAPI": _MOLT_NUMPY_UFUNC_API_CAPSULE,
}
_SOURCE_EXTENSION_PLAN_KINDS = {"meson-intro-targets"}
_SOURCE_EXTENSION_SOURCE_SUFFIXES = {
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".c++",
    ".m",
    ".mm",
}
_SOURCE_EXTENSION_TARGET_OUTPUT_SUFFIXES = (
    ".molt.wasm",
    ".pyd",
    ".so",
    ".dll",
    ".dylib",
)
_NINJA_OBJECT_SUFFIXES = (".o", ".obj")
_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX = (
    "extension_manifest.json source missing:"
)
_MESON_EXTENSION_TARGET_TYPES = {"shared module", "shared library", "library"}


@dataclass(frozen=True)
class _SourceExtensionDependencyFact:
    path: Path
    sha256: str

    def manifest_payload(self) -> dict[str, str]:
        return {"path": str(self.path), "sha256": self.sha256}


@dataclass(frozen=True)
class _SourceExtensionArtifactSymbolInspection:
    defined_symbols: frozenset[str]
    undefined_symbols: frozenset[str]
    defined_function_symbols: frozenset[str]
    symbol_authority: str
    wasm_imports: tuple[WasmImport, ...] | None
    wasm_function_import_signatures: tuple[tuple[str, str, tuple[str, ...], str], ...]
    wasm_function_exports: tuple[str, ...] = ()
    artifact_bytes: bytes | None = None
    artifact_digest: str | None = None
    wasm_interface: WasmRelocatableObjectInterface | None = None


@dataclass(frozen=True)
class _SourceExtensionCompileUnitIdentity:
    owner_target_id: str
    producer_object_path: Path


@dataclass(frozen=True)
class _SourceExtensionObjectFact:
    source_path: Path
    language: SourceExtensionLanguage
    object_path: Path
    source_sha256: str
    object_sha256: str
    defined_symbols: tuple[str, ...]
    undefined_symbols: tuple[str, ...]
    defined_function_symbols: tuple[str, ...]
    compile_command: tuple[str, ...]
    symbol_authority: str
    symbol_command: tuple[str, ...]
    dependencies: tuple[_SourceExtensionDependencyFact, ...] = ()
    producer_unit: _SourceExtensionCompileUnitIdentity | None = None

    def manifest_payload(
        self,
        *,
        required_c_api_symbols: Sequence[str] = (),
        required_capsules: Sequence[str] = (),
        project_generated_c_api_symbols: Sequence[str] = (),
    ) -> dict[str, Any]:
        payload = {
            "source": str(self.source_path),
            "language": self.language.value,
            "object": self.object_path.name,
            "source_sha256": self.source_sha256,
            "object_sha256": self.object_sha256,
            "defined_symbols": list(self.defined_symbols),
            "undefined_symbols": list(self.undefined_symbols),
            "compile_command": list(self.compile_command),
            "symbol_authority": self.symbol_authority,
            "dependencies": [
                dependency.manifest_payload() for dependency in self.dependencies
            ],
            "required_c_api_symbols": list(required_c_api_symbols),
            "required_capsules": list(required_capsules),
            "project_generated_c_api_symbols": list(project_generated_c_api_symbols),
        }
        if self.symbol_command:
            payload["symbol_command"] = list(self.symbol_command)
        if self.producer_unit is not None:
            payload["producer_unit"] = {
                "target_id": self.producer_unit.owner_target_id,
                "object": self.producer_unit.producer_object_path.as_posix(),
            }
        return payload


@dataclass(frozen=True)
class _SourceExtensionObjectClosure:
    init_symbol: str
    init_symbol_owner: _SourceExtensionObjectFact
    objects: tuple[_SourceExtensionObjectFact, ...]
    undefined_symbols: tuple[str, ...]

    def manifest_payload(
        self,
        *,
        defined_symbols: Sequence[str] | None = None,
        undefined_symbols: Sequence[str] | None = None,
        wasm_imports: Sequence[Mapping[str, str]] | None = None,
        runtime_symbols: Sequence[str] | None = None,
        required_c_api_by_object: Mapping[Path, Sequence[str]] | None = None,
        required_capsules_by_object: Mapping[Path, Sequence[str]] | None = None,
        project_generated_c_api_by_object: Mapping[Path, Sequence[str]] | None = None,
        project_generated_c_api_prefixes: Sequence[str] = (),
    ) -> dict[str, Any]:
        c_api_by_object = required_c_api_by_object or {}
        capsules_by_object = required_capsules_by_object or {}
        generated_by_object = project_generated_c_api_by_object or {}
        required_capsules = sorted(
            {
                capsule
                for fact in self.objects
                for capsule in capsules_by_object.get(fact.object_path.resolve(), ())
            }
        )
        required_c_api_symbols = sorted(
            {
                symbol
                for fact in self.objects
                for symbol in c_api_by_object.get(fact.object_path.resolve(), ())
            }
        )
        project_generated_c_api_symbols = sorted(
            {
                symbol
                for fact in self.objects
                for symbol in generated_by_object.get(fact.object_path.resolve(), ())
            }
        )
        return {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": self.init_symbol,
            "init_symbol_owner": self.init_symbol_owner.object_path.name,
            "defined_symbols": list(
                sorted(
                    {symbol for fact in self.objects for symbol in fact.defined_symbols}
                )
                if defined_symbols is None
                else defined_symbols
            ),
            "undefined_symbols": list(
                self.undefined_symbols
                if undefined_symbols is None
                else undefined_symbols
            ),
            "runtime_symbols": list(
                self.undefined_symbols if runtime_symbols is None else runtime_symbols
            ),
            **(
                {"wasm_imports": [dict(item) for item in wasm_imports]}
                if wasm_imports is not None
                else {}
            ),
            "required_capsules": required_capsules,
            "required_c_api_symbols": required_c_api_symbols,
            "project_generated_c_api_symbols": project_generated_c_api_symbols,
            "project_generated_c_api_prefixes": list(
                sorted(project_generated_c_api_prefixes)
            ),
            "objects": [
                fact.manifest_payload(
                    required_c_api_symbols=tuple(
                        c_api_by_object.get(fact.object_path.resolve(), ())
                    ),
                    required_capsules=tuple(
                        capsules_by_object.get(fact.object_path.resolve(), ())
                    ),
                    project_generated_c_api_symbols=tuple(
                        generated_by_object.get(fact.object_path.resolve(), ())
                    ),
                )
                for fact in self.objects
            ],
        }


@dataclass(frozen=True)
class _SourceExtensionCAPIRequirements:
    required_by_object: dict[Path, tuple[str, ...]]
    required_capsules_by_object: dict[Path, tuple[str, ...]]
    project_generated_c_api_by_object: dict[Path, tuple[str, ...]]
    project_generated_c_api_prefixes: tuple[str, ...]
    project_defined_symbols: tuple[str, ...]
    missing_symbols: tuple[str, ...]
    fail_fast_symbols: tuple[str, ...]

    def manifest_payload(self) -> dict[str, Any]:
        project_generated_symbols = sorted(
            {
                symbol
                for symbols in self.project_generated_c_api_by_object.values()
                for symbol in symbols
            }
        )
        return {
            "required_symbol_count": sum(
                len(symbols) for symbols in self.required_by_object.values()
            ),
            "required_capsule_count": sum(
                len(capsules) for capsules in self.required_capsules_by_object.values()
            ),
            "project_defined_symbol_count": len(self.project_defined_symbols),
            "project_generated_symbol_count": len(project_generated_symbols),
            "missing_symbol_count": len(self.missing_symbols),
            "fail_fast_symbol_count": len(self.fail_fast_symbols),
            "project_generated_c_api_prefixes": list(
                self.project_generated_c_api_prefixes
            ),
            "project_generated_c_api_symbols": project_generated_symbols,
            "missing_symbols": list(self.missing_symbols),
            "fail_fast_symbols": list(self.fail_fast_symbols),
        }


@dataclass(frozen=True)
class _SourceExtensionCompileUnit:
    source_path: Path
    owner_target_id: str
    producer_object_path: Path
    generated: bool
    language: SourceExtensionLanguage
    compiler: tuple[str, ...]
    include_dirs: tuple[Path, ...]
    compile_args: tuple[str, ...]
    force_include: bool = False

    def manifest_payload(self, *, build_root: Path) -> dict[str, Any]:
        return {
            "source": str(self.source_path),
            "owner_target_id": self.owner_target_id,
            "producer_object_path": self.producer_object_path.resolve()
            .relative_to(build_root.resolve())
            .as_posix(),
            "force_include": self.force_include,
            "generated": self.generated,
            "language": self.language,
            "compiler": list(self.compiler),
            "include_dirs": [str(path) for path in self.include_dirs],
            "compile_args": list(self.compile_args),
        }


@dataclass(frozen=True)
class _SourceExtensionBuildPlan:
    kind: str
    plan_path: Path
    plan_sha256: str
    compile_commands_path: Path | None
    compile_commands_sha256: str | None
    target_id: str
    target_name: str
    target_selector: str
    target_type: str
    source_root: Path
    build_root: Path
    sources: tuple[Path, ...]
    generated_sources: tuple[Path, ...]
    skipped_generated_sources: tuple[Path, ...]
    non_compiled_inputs: tuple[Path, ...]
    compile_units: tuple[_SourceExtensionCompileUnit, ...]
    include_dirs: tuple[Path, ...]
    compile_args: tuple[str, ...]
    link_args: tuple[str, ...]
    digest: str
    consumed_forced_link_args: tuple[str, ...] = ()
    producer_link_args: tuple[str, ...] = ()
    lazy_static_target_ids: tuple[str, ...] = ()
    dependencies_path: Path | None = None
    dependencies_sha256: str | None = None
    python_provider: Mapping[str, Any] | None = None

    def manifest_payload(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "plan": str(self.plan_path),
            "plan_sha256": self.plan_sha256,
            "compile_commands": (
                str(self.compile_commands_path)
                if self.compile_commands_path is not None
                else None
            ),
            "compile_commands_sha256": self.compile_commands_sha256,
            "target_id": self.target_id,
            "target_name": self.target_name,
            "target_selector": self.target_selector,
            "target_type": self.target_type,
            "source_root": str(self.source_root),
            "build_root": str(self.build_root),
            "digest": self.digest,
            "sources": [str(path) for path in self.sources],
            "generated_sources": [str(path) for path in self.generated_sources],
            "skipped_generated_sources": [
                str(path) for path in self.skipped_generated_sources
            ],
            "non_compiled_inputs": [str(path) for path in self.non_compiled_inputs],
            "compile_units": [
                unit.manifest_payload(build_root=self.build_root)
                for unit in self.compile_units
            ],
            "include_dirs": [str(path) for path in self.include_dirs],
            "compile_args": list(self.compile_args),
            "link_args": list(self.link_args),
            "consumed_forced_link_args": list(self.consumed_forced_link_args),
            "producer_link_args": list(self.producer_link_args),
            "lazy_static_target_ids": list(self.lazy_static_target_ids),
            "dependencies": str(self.dependencies_path)
            if self.dependencies_path
            else None,
            "dependencies_sha256": self.dependencies_sha256,
            "python_provider": self.python_provider,
        }

    def source_paths(self) -> tuple[Path, ...]:
        return (*self.sources, *self.generated_sources)


def _resolve_source_extension_plan_path(*, base: Path, raw_path: Any) -> Path:
    path = Path(str(raw_path)).expanduser()
    if not path.is_absolute():
        path = (base / path).absolute()
    return path.resolve()


def _source_extension_plan_target_selector(
    *,
    module_name: str,
    selector: Any,
) -> str:
    if isinstance(selector, str) and selector.strip():
        return selector.strip()
    return module_name.rsplit(".", 1)[-1]


def _dedupe_paths(paths: Sequence[Path]) -> tuple[Path, ...]:
    seen: set[Path] = set()
    deduped: list[Path] = []
    for path in paths:
        resolved = path.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        deduped.append(resolved)
    return tuple(deduped)


def _resolve_meson_plan_artifact_path(
    raw_path: Any,
    *,
    source_root: Path,
    build_root: Path,
    prefer_build_root: bool,
) -> Path:
    path = Path(str(raw_path)).expanduser()
    if path.is_absolute():
        return path.resolve()
    candidates = (
        (build_root / path, source_root / path)
        if prefer_build_root
        else (source_root / path, build_root / path)
    )
    for candidate in candidates:
        if candidate.exists():
            return candidate.resolve()
    return candidates[0].resolve()


def _is_compilable_source_path(path: Path) -> bool:
    return path.suffix.lower() in _SOURCE_EXTENSION_SOURCE_SUFFIXES


def _meson_link_args(target: Mapping[str, Any]) -> tuple[str, ...]:
    """One ordered operand view, with equal mirrored metadata admitted once."""

    views: list[tuple[str, ...]] = []

    def add_view(raw: Any, *, field: str) -> None:
        if raw is None:
            return
        if not isinstance(raw, list) or any(
            not isinstance(argument, str) or not argument.strip() for argument in raw
        ):
            raise ValueError(f"Meson {field} must be a list of non-empty strings")
        if raw:
            views.append(tuple(raw))

    for field in ("linker_parameters", "link_args"):
        add_view(target.get(field), field=field)
    groups = target.get("target_sources")
    if isinstance(groups, list):
        for group in groups:
            if not isinstance(group, Mapping) or "linker" not in group:
                continue
            linker = group["linker"]
            if (
                not isinstance(linker, list)
                or not linker
                or any(not isinstance(part, str) or not part.strip() for part in linker)
            ):
                raise ValueError(
                    "Meson nested linker command must be a non-empty string list"
                )
            if "compiler" in group:
                raise ValueError(
                    "Meson source group mixes compiler and linker authority"
                )
            add_view(group.get("parameters"), field="nested linker parameters")
    if any(view != views[0] for view in views[1:]):
        raise ValueError("Meson linker operand views disagree in values or order")
    return views[0] if views else ()


def _meson_target_filename_names(filename: Any) -> set[str]:
    raw_filenames = filename if isinstance(filename, list) else (filename,)
    names: set[str] = set()
    for raw_filename in raw_filenames:
        if not isinstance(raw_filename, str):
            continue
        basename = Path(raw_filename).name
        lowered = basename.lower()
        stripped = basename
        for suffix in _SOURCE_EXTENSION_TARGET_OUTPUT_SUFFIXES:
            if lowered.endswith(suffix):
                stripped = basename[: -len(suffix)]
                break
        names.add(stripped)
        names.add(Path(basename).stem)
        if "." in stripped:
            names.add(stripped.split(".", 1)[0])
    return {name for name in names if name}


def _meson_extension_targets_by_selector(
    payload: Sequence[Any],
) -> dict[str, tuple[Mapping[str, Any], ...]]:
    """One alias/ambiguity authority for live plans and published receipts."""
    index: dict[str, list[Mapping[str, Any]]] = {}
    target_ids: set[str] = set()
    for target in payload:
        if not isinstance(target, Mapping):
            raise ValueError("Meson intro-targets entry is not an object")
        target_id = target.get("id")
        if not isinstance(target_id, str) or not target_id or target_id in target_ids:
            raise ValueError("Meson target identity is missing or duplicated")
        target_ids.add(target_id)
        target_type = target.get("type")
        if (
            not isinstance(target_type, str)
            or target_type not in _MESON_EXTENSION_TARGET_TYPES
        ):
            continue
        name = target.get("name")
        if not isinstance(name, str) or not name:
            raise ValueError("Meson extension target has no non-empty name")
        for alias in {
            target_id,
            name,
            *_meson_target_filename_names(target.get("filename")),
        }:
            index.setdefault(alias, []).append(target)
    return {alias: tuple(targets) for alias, targets in index.items()}


def _meson_target_output_paths(filename: Any, *, build_root: Path) -> tuple[Path, ...]:
    raw_filenames = filename if isinstance(filename, list) else (filename,)
    outputs: list[Path] = []
    for raw_filename in raw_filenames:
        if not isinstance(raw_filename, str) or not raw_filename.strip():
            continue
        target_path = Path(raw_filename.replace("\\", "/")).expanduser()
        if not target_path.is_absolute():
            target_path = build_root / target_path
        outputs.append(target_path.resolve())
    return _dedupe_paths(outputs)


def _meson_target_object_roots(filename: Any, *, build_root: Path) -> tuple[Path, ...]:
    roots: list[Path] = []
    for target_path in _meson_target_output_paths(filename, build_root=build_root):
        roots.append((target_path.parent / f"{target_path.name}.p").resolve())
    return _dedupe_paths(roots)


def _source_extension_build_plan_digest(plan: _SourceExtensionBuildPlan) -> str:
    payload = {
        "kind": plan.kind,
        "plan_sha256": plan.plan_sha256,
        "compile_commands_sha256": plan.compile_commands_sha256,
        "target_id": plan.target_id,
        "target_name": plan.target_name,
        "target_selector": plan.target_selector,
        "target_type": plan.target_type,
        "source_root": str(plan.source_root),
        "build_root": str(plan.build_root),
        "sources": [str(path) for path in plan.sources],
        "generated_sources": [str(path) for path in plan.generated_sources],
        "skipped_generated_sources": [
            str(path) for path in plan.skipped_generated_sources
        ],
        "non_compiled_inputs": [str(path) for path in plan.non_compiled_inputs],
        "compile_units": [
            unit.manifest_payload(build_root=plan.build_root)
            for unit in plan.compile_units
        ],
        "include_dirs": [str(path) for path in plan.include_dirs],
        "compile_args": list(plan.compile_args),
        "link_args": list(plan.link_args),
        "consumed_forced_link_args": list(plan.consumed_forced_link_args),
        "producer_link_args": list(plan.producer_link_args),
        "lazy_static_target_ids": list(plan.lazy_static_target_ids),
        "dependencies_sha256": plan.dependencies_sha256,
        "python_provider": plan.python_provider,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _resolve_compile_command_path(raw_path: Any, *, directory: Path) -> Path:
    path = Path(str(raw_path)).expanduser()
    if not path.is_absolute():
        path = directory / path
    return path.resolve()


def _compile_command_arguments(entry: Mapping[str, Any]) -> list[str] | None:
    raw_arguments = entry.get("arguments")
    if isinstance(raw_arguments, list):
        return [str(arg) for arg in raw_arguments]
    raw_command = entry.get("command")
    if isinstance(raw_command, str) and raw_command.strip():
        if os.name == "nt":
            return _split_windows_command_line(raw_command)
        return shlex.split(raw_command, posix=True)
    return None


def _path_basename_token(raw: str) -> str:
    return Path(raw).name.lower().removesuffix(".exe")


def _compile_command_compiler_and_args(
    arguments: Sequence[str],
) -> tuple[tuple[str, ...], list[str]]:
    if not arguments:
        return (), []
    items = [str(arg) for arg in arguments]
    idx = 0
    while idx + 1 < len(items) and _path_basename_token(items[idx]) in {
        "ccache",
        "sccache",
        "distcc",
    }:
        idx += 1

    compiler_end = idx + 1
    compiler = _path_basename_token(items[idx])
    if compiler == "zig" and idx + 1 < len(items):
        subcommand = items[idx + 1]
        if subcommand in {"cc", "c++"}:
            compiler_end = idx + 2
    return tuple(items[:compiler_end]), items[compiler_end:]


def _split_windows_command_line(command: str) -> list[str] | None:
    argv: list[str] = []
    arg: list[str] = []
    in_quotes = False
    token_started = False
    idx = 0
    while idx < len(command):
        char = command[idx]
        if char in {" ", "\t"} and not in_quotes:
            if token_started:
                argv.append("".join(arg))
                arg = []
                token_started = False
            idx += 1
            continue
        if char == "\\":
            slash_start = idx
            while idx < len(command) and command[idx] == "\\":
                idx += 1
            slash_count = idx - slash_start
            if idx < len(command) and command[idx] == '"':
                arg.extend("\\" * (slash_count // 2))
                if slash_count % 2 == 0:
                    in_quotes = not in_quotes
                else:
                    arg.append('"')
                token_started = True
                idx += 1
                continue
            arg.extend("\\" * slash_count)
            token_started = True
            continue
        if char == '"':
            in_quotes = not in_quotes
            token_started = True
            idx += 1
            continue
        arg.append(char)
        token_started = True
        idx += 1
    if in_quotes:
        return None
    if token_started:
        argv.append("".join(arg))
    return argv


def _reject_unowned_precompiled_input(token: str) -> None:
    if token.startswith(
        (
            "/Fp",
            "/Yu",
            "/Yc",
            "/FU",
            "-include-pch",
            "-include-pth",
            "-fmodule-file=",
            "-fprebuilt-module-path=",
        )
    ):
        raise ValueError(
            f"source-extension compiler option {token!r} requires explicit "
            "precompiled-header/module input custody"
        )


def _compile_command_semantic_args(
    arguments: Sequence[str],
    *,
    source_path: Path,
    directory: Path,
) -> list[str]:
    if not arguments:
        return []
    _compiler, args = _compile_command_compiler_and_args(arguments)
    semantic_args: list[str] = []
    source_seen = False
    per_file_language: str | None = None
    for span in compiler_argument_spans(args):
        arg = span.option
        if span.context in {"opaque", "cc1"}:
            semantic_args.extend(span.raw)
            continue
        if span.context == "driver":
            _reject_unowned_precompiled_input(arg)
            if span.is_output or arg in {"-c", "/c", "--"}:
                continue
            if arg.startswith("-x"):
                if not source_seen:
                    semantic_args.extend(span.raw)
                continue
            if arg.startswith(("/Tc", "/Tp")):
                raw_source = arg[3:] if len(arg) > 3 else span.arguments[1]
                if (
                    _resolve_compile_command_path(raw_source, directory=directory)
                    == source_path
                ):
                    per_file_language = "c" if arg.startswith("/Tc") else "c++"
                    source_seen = True
                continue
        # Only standalone positional operands can identify the source. A macro,
        # include path or opaque argument that equals it is still an operand.
        if len(span.raw) == 1:
            try:
                if (
                    _resolve_compile_command_path(arg, directory=directory)
                    == source_path
                ):
                    source_seen = True
                    continue
            except OSError:
                pass
        if span.context == "positional":
            raise ValueError(
                f"source-extension compile command has extra positional input {arg!r}"
            )
        semantic_args.extend(span.raw)
    if per_file_language is not None:
        # /Tc and /Tp override global /TC and /TP regardless of order.
        semantic_args.extend(("-x", per_file_language))
    return semantic_args


def _compile_command_output_path(
    arguments: Sequence[str],
    *,
    directory: Path,
) -> Path | None:
    if not arguments:
        return None
    _compiler, args = _compile_command_compiler_and_args(arguments)
    output_path: Path | None = None
    for span in compiler_argument_spans(args):
        option = span.output_option
        if option in {"-o", "/Fo"}:
            value = (
                span.arguments[1]
                if span.option == option
                else span.option[len(option) :]
            )
            output_path = _resolve_compile_command_path(value, directory=directory)
    return output_path


def _path_is_within(path: Path, parent: Path) -> bool:
    """Compare paths already resolved at Meson/compile-database ingestion.

    Resolving again in every owner/member comparison performs filesystem I/O
    proportional to graph edges times targets. Physical custody is established
    at ingestion; containment itself is a lexical operation on that snapshot.
    """
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


def _compile_command_args_and_include_dirs(
    arguments: Sequence[str],
    *,
    directory: Path,
) -> tuple[tuple[str, ...], tuple[Path, ...]]:
    compile_args: list[str] = []
    include_dirs: list[Path] = []
    for span in compiler_argument_spans(arguments):
        item = span.option
        if span.context != "driver":
            compile_args.extend(span.raw)
            continue
        if item in {"-I", "/I"}:
            include_dirs.append(
                _resolve_compile_command_path(span.arguments[1], directory=directory)
            )
        elif item.startswith(("-I", "/I")) and len(item) > 2:
            include_dirs.append(
                _resolve_compile_command_path(item[2:], directory=directory)
            )
        elif item in {
            "-isystem",
            "-iquote",
            "-include",
            "-imacros",
            "-idirafter",
            "/FI",
        }:
            compile_args.extend(
                (
                    span.raw[0],
                    str(
                        _resolve_compile_command_path(
                            span.arguments[1], directory=directory
                        )
                    ),
                )
            )
        elif item.startswith("/FI") and len(item) > 3:
            compile_args.extend(
                (
                    "/FI",
                    str(_resolve_compile_command_path(item[3:], directory=directory)),
                )
            )
        else:
            compile_args.extend(span.raw)
    return tuple(compile_args), _dedupe_paths(include_dirs)


@dataclass(frozen=True)
class _CompileCommandUnit:
    source_path: Path
    object_path: Path
    compiler: tuple[str, ...]
    compile_args: tuple[str, ...]
    include_dirs: tuple[Path, ...]


def _load_compile_command_units(
    compile_commands_path: Path,
    *,
    required_sources: set[Path] | None = None,
    target_output_roots: Sequence[Path] = (),
) -> tuple[
    dict[Path, _CompileCommandUnit] | None,
    list[str],
]:
    if not compile_commands_path.exists() or not compile_commands_path.is_file():
        return None, [
            "Meson source-extension builds require compile_commands.json for "
            f"actual per-source compile arguments: {compile_commands_path}"
        ]
    try:
        payload = json.loads(compile_commands_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        return None, [
            f"failed to read compile_commands.json {compile_commands_path}: {exc}"
        ]
    if not isinstance(payload, list):
        return None, [
            f"compile_commands.json must be a JSON array: {compile_commands_path}"
        ]

    required_source_paths = (
        {source_path.resolve() for source_path in required_sources}
        if required_sources is not None
        else None
    )
    owned_output_roots = _dedupe_paths([root.resolve() for root in target_output_roots])
    commands_by_output: dict[Path, _CompileCommandUnit] = {}
    errors: list[str] = []
    for entry in payload:
        if not isinstance(entry, Mapping):
            continue
        raw_directory = entry.get("directory")
        directory = (
            Path(str(raw_directory)).expanduser().resolve()
            if isinstance(raw_directory, str) and raw_directory.strip()
            else compile_commands_path.parent.resolve()
        )
        raw_file = entry.get("file")
        if not isinstance(raw_file, str) or not raw_file.strip():
            if required_source_paths is None:
                errors.append("compile_commands.json entry is missing non-empty 'file'")
            continue
        source_path = _resolve_compile_command_path(raw_file, directory=directory)
        arguments = _compile_command_arguments(entry)
        if arguments is None:
            errors.append(f"compile command for {source_path} lacks arguments/command")
            continue
        compiler, _compiler_args = _compile_command_compiler_and_args(arguments)
        try:
            command_output_path = _compile_command_output_path(
                arguments, directory=directory
            )
        except ValueError as exc:
            errors.append(f"compile command for {source_path}: {exc}")
            continue
        raw_output = entry.get("output")
        declared_output_path = (
            _resolve_compile_command_path(raw_output, directory=directory)
            if isinstance(raw_output, str) and raw_output.strip()
            else None
        )
        if (
            command_output_path is not None
            and declared_output_path is not None
            and command_output_path != declared_output_path
        ):
            errors.append(
                "compile_commands.json output disagrees with the compiler command "
                f"for {source_path}: {declared_output_path} != {command_output_path}"
            )
            continue
        output_path = declared_output_path or command_output_path
        if output_path is None:
            errors.append(
                f"compile_commands.json entry has no object output for {source_path}"
            )
            continue
        if owned_output_roots and not any(
            _path_is_within(output_path, root) for root in owned_output_roots
        ):
            continue
        try:
            semantic_args = _compile_command_semantic_args(
                arguments,
                source_path=source_path,
                directory=directory,
            )
            compile_args, include_dirs = _compile_command_args_and_include_dirs(
                semantic_args,
                directory=directory,
            )
        except ValueError as exc:
            errors.append(f"compile command for {source_path}: {exc}")
            continue
        unit = _CompileCommandUnit(
            source_path=source_path,
            object_path=output_path,
            compiler=compiler,
            compile_args=compile_args,
            include_dirs=include_dirs,
        )
        existing = commands_by_output.get(output_path)
        if existing is not None and existing != unit:
            errors.append(
                "compile_commands.json has conflicting commands for object "
                f"{output_path}: {existing.source_path} and {source_path}"
            )
            continue
        commands_by_output[output_path] = unit
    if errors:
        return None, errors
    return {
        path: commands_by_output[path]
        for path in sorted(commands_by_output)
        if required_source_paths is None
        or commands_by_output[path].source_path in required_source_paths
    }, []


def _ninja_logical_lines(path: Path) -> tuple[str, ...]:
    try:
        raw_lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError):
        return ()
    lines: list[str] = []
    pending = ""
    for raw_line in raw_lines:
        line = raw_line.rstrip()
        if pending:
            pending += line.lstrip()
        else:
            pending = line
        if pending.rstrip().endswith("$"):
            pending = pending.rstrip()[:-1] + " "
            continue
        lines.append(pending)
        pending = ""
    if pending:
        lines.append(pending)
    return tuple(lines)


def _ninja_find_build_colon(text: str) -> int:
    escaped = False
    for index, char in enumerate(text):
        if escaped:
            escaped = False
            continue
        if char == "$":
            escaped = True
            continue
        if char == ":":
            return index
    return -1


def _ninja_split_words(text: str) -> tuple[str, ...]:
    words: list[str] = []
    current: list[str] = []
    index = 0
    while index < len(text):
        char = text[index]
        if char.isspace():
            if current:
                words.append("".join(current))
                current = []
            index += 1
            continue
        if char == "$" and index + 1 < len(text):
            next_char = text[index + 1]
            if next_char in {" ", ":", "$"}:
                current.append(next_char)
                index += 2
                continue
        current.append(char)
        index += 1
    if current:
        words.append("".join(current))
    return tuple(words)


def _resolve_ninja_build_path(raw_path: str, *, build_root: Path) -> Path:
    path = Path(raw_path).expanduser()
    if not path.is_absolute():
        path = build_root / path
    return path.resolve()


def _load_ninja_build_inputs(
    build_root: Path, *, include_implicit: bool
) -> dict[Path, tuple[Path, ...]]:
    edges: dict[Path, tuple[Path, ...]] = {}
    for line in _ninja_logical_lines(build_root / "build.ninja"):
        if not line.startswith("build "):
            continue
        payload = line[len("build ") :]
        colon_index = _ninja_find_build_colon(payload)
        if colon_index < 0:
            continue
        output_words = _ninja_split_words(payload[:colon_index])
        rule_and_inputs = _ninja_split_words(payload[colon_index + 1 :])
        if not output_words or not rule_and_inputs:
            continue
        raw_inputs: list[str] = []
        for word in rule_and_inputs[1:]:
            if word in {"|", "||"}:
                if include_implicit:
                    continue
                break
            raw_inputs.append(word)
        inputs = tuple(
            _resolve_ninja_build_path(raw_input, build_root=build_root)
            for raw_input in raw_inputs
        )
        for output_word in output_words:
            output = _resolve_ninja_build_path(output_word, build_root=build_root)
            edges[output] = inputs
    return edges


def _load_ninja_build_explicit_inputs(
    build_root: Path,
) -> dict[Path, tuple[Path, ...]]:
    return _load_ninja_build_inputs(build_root, include_implicit=False)


def _load_ninja_build_all_inputs(
    build_root: Path,
) -> dict[Path, tuple[Path, ...]]:
    """Return explicit, implicit, and order-only Ninja dependency edges."""

    return _load_ninja_build_inputs(build_root, include_implicit=True)


@dataclass(frozen=True)
class _MesonStaticLibraryProjection:
    targets: tuple[Mapping[str, Any], ...]
    excluded_targets: tuple[Mapping[str, Any], ...]
    forced_target_ids: frozenset[str]
    link_args: tuple[str, ...]
    consumed_forced_args: tuple[str, ...]
    producer_link_args: tuple[str, ...]
    lazy_static_target_ids: tuple[str, ...]


class _MesonOutputIdentity:
    """Declared output ownership; suffixes never establish static-library custody."""

    def __init__(self, payload: Sequence[Any], *, build_root: Path) -> None:
        self.build_root = build_root
        self.outputs: dict[Path, Mapping[str, Any]] = {}
        self.basenames: dict[str, list[Mapping[str, Any]]] = {}
        self.aliases: dict[str, list[Mapping[str, Any]]] = {}
        target_ids: set[str] = set()
        for target in payload:
            if not isinstance(target, Mapping):
                continue
            target_id = str(target.get("id", "")).strip()
            if not target_id or target_id in target_ids:
                raise ValueError(
                    f"Meson target identity is missing or duplicated: {target_id!r}"
                )
            target_ids.add(target_id)
            for output in _meson_target_output_paths(
                target.get("filename"), build_root=build_root
            ):
                previous = self.outputs.get(output)
                if previous is not None and previous is not target:
                    raise ValueError(
                        f"Meson output has multiple declared owners: {output}"
                    )
                self.outputs[output] = target
                self._add(self.basenames, output.name, target)
                if str(target.get("type", "")).strip() == "static library":
                    for alias in (
                        output.name,
                        output.stem,
                        os.path.normcase(output.stem).removeprefix("lib"),
                        str(target.get("id", "")),
                        str(target.get("name", "")),
                    ):
                        self._add(self.aliases, alias, target)

    @staticmethod
    def _add(
        index: dict[str, list[Mapping[str, Any]]],
        key: str,
        target: Mapping[str, Any],
    ) -> None:
        if not key:
            return
        values = index.setdefault(os.path.normcase(key), [])
        if not any(value is target for value in values):
            values.append(target)

    @staticmethod
    def _unique(
        matches: Sequence[Mapping[str, Any]], *, operand: str
    ) -> Mapping[str, Any] | None:
        if len(matches) > 1:
            names = ", ".join(str(target.get("id", "")) for target in matches)
            raise ValueError(
                f"Meson output identity {operand!r} is ambiguous ({names})"
            )
        return matches[0] if matches else None

    def resolve(
        self, operand: str, *, exclusion: bool = False
    ) -> Mapping[str, Any] | None:
        normalized = operand.replace("\\", "/")
        path = Path(normalized).expanduser()
        qualified = "/" in normalized or path.is_absolute()
        resolved = (path if path.is_absolute() else self.build_root / path).resolve()
        exact = self.outputs.get(resolved)
        if exact is not None or qualified:
            return exact
        matches = (self.aliases if exclusion else self.basenames).get(
            os.path.normcase(normalized), ()
        )
        return self._unique(matches, operand=operand)


def _meson_static_library_projection(
    *,
    primary_target: Mapping[str, Any],
    payload: Sequence[Any],
    build_root: Path,
    exclude_linked_static_libraries: Sequence[str] = (),
) -> _MesonStaticLibraryProjection:
    """Fold only metadata-owned outputs and preserve ordered external operands.

    Forced archives become explicit source-object roots. Lazy target identities
    remain as provenance for the canonical typed final-link admission gate.
    """
    identity = _MesonOutputIdentity(payload, build_root=build_root)
    linked: list[Mapping[str, Any]] = []
    linked_ids: set[int] = set()
    forced_ids: set[int] = set()
    excluded_ids: set[int] = set()
    for exclusion in exclude_linked_static_libraries:
        if not isinstance(exclusion, str) or not exclusion.strip():
            raise ValueError(
                "Meson static-library exclusions must be non-empty strings"
            )
        excluded = identity.resolve(exclusion, exclusion=True)
        if excluded is None:
            raise ValueError(
                f"Meson static-library exclusion has no declared owner: {exclusion!r}"
            )
        if str(excluded.get("type", "")).strip() != "static library":
            raise ValueError(
                f"Meson exclusion is not a static-library target: {exclusion!r}"
            )
        excluded_ids.add(id(excluded))

    def append_target(target: Mapping[str, Any], *, forced: bool) -> None:
        if id(target) not in linked_ids:
            linked_ids.add(id(target))
            linked.append(target)
        if forced:
            forced_ids.add(id(target))

    retained: list[str] = []
    consumed_forced_args: list[str] = []
    whole_archive = False
    producer_link_args = _meson_link_args(primary_target)
    for span in source_extension_link_arguments(producer_link_args):
        if span.kind == "product":
            continue
        if span.kind == "framework":
            retained.extend(span.arguments)
            continue
        if span.kind == "scope" and span.value == "--whole-archive":
            if whole_archive:
                raise ValueError("Meson whole-archive scopes cannot be nested")
            whole_archive = True
            retained.extend(span.arguments)
            continue
        if span.kind == "scope" and span.value == "--no-whole-archive":
            if not whole_archive:
                raise ValueError("Meson whole-archive end has no start")
            whole_archive = False
            retained.extend(span.arguments)
            continue
        # Search directives and linker flags are not positive output custody.
        explicit_input = span.kind in {"forced", "input"}
        owner = identity.resolve(span.value) if explicit_input else None
        if owner is not None and str(owner.get("type", "")).strip() == "static library":
            append_target(owner, forced=whole_archive or span.kind == "forced")
            if span.kind == "forced":
                consumed_forced_args.extend(span.arguments)
            continue
        retained.extend(span.arguments)
    if whole_archive:
        raise ValueError("Meson whole-archive start has no end")

    # Aggregate archives carry object ownership in Ninja, not intro source lists.
    static_targets = [
        target
        for target in payload
        if isinstance(target, Mapping)
        and str(target.get("type", "")).strip() == "static library"
    ]
    archive_edges = _load_ninja_build_explicit_inputs(build_root)
    object_roots = [
        (root, target)
        for target in static_targets
        for root in _meson_target_object_roots(
            target.get("filename"), build_root=build_root
        )
    ]
    queue = list(linked)
    expanded: set[tuple[int, bool]] = set()
    for target in queue:
        forced = id(target) in forced_ids
        state = (id(target), forced)
        if state in expanded or id(target) in excluded_ids:
            continue
        expanded.add(state)
        outputs = _meson_target_output_paths(
            target.get("filename"), build_root=build_root
        )
        own_roots = _meson_target_object_roots(
            target.get("filename"), build_root=build_root
        )
        declared_sources = any(
            _is_compilable_source_path(Path(str(source)))
            for group in target.get("target_sources") or ()
            if isinstance(group, Mapping)
            for field in ("sources", "generated_sources")
            for source in group.get(field) or ()
        )
        if (
            forced
            and not declared_sources
            and not any(archive_edges.get(output) for output in outputs)
        ):
            raise ValueError(
                "Meson forced aggregate has neither declared source members nor "
                "Ninja member custody: " + ", ".join(str(output) for output in outputs)
            )
        for output in outputs:
            for member in archive_edges.get(output, ()):
                if member.suffix.lower() not in _NINJA_OBJECT_SUFFIXES:
                    continue
                if any(_path_is_within(member, root) for root in own_roots):
                    if forced and not declared_sources:
                        raise ValueError(
                            "Meson forced member lacks declared source custody: "
                            + str(member)
                        )
                    continue
                owners: list[Mapping[str, Any]] = []
                for root, candidate in object_roots:
                    if _path_is_within(member, root) and not any(
                        owner is candidate for owner in owners
                    ):
                        owners.append(candidate)
                owner = identity._unique(owners, operand=str(member))
                if owner is None:
                    if forced:
                        raise ValueError(
                            "Meson forced member has no declared source owner: "
                            + str(member)
                        )
                    continue
                append_target(owner, forced=forced)
                queue.append(owner)

    targets = tuple(target for target in linked if id(target) not in excluded_ids)
    return _MesonStaticLibraryProjection(
        targets=targets,
        excluded_targets=tuple(
            target for target in linked if id(target) in excluded_ids
        ),
        forced_target_ids=frozenset(
            str(target["id"]) for target in targets if id(target) in forced_ids
        ),
        link_args=tuple(retained),
        consumed_forced_args=tuple(consumed_forced_args),
        producer_link_args=producer_link_args,
        lazy_static_target_ids=tuple(
            str(target["id"]) for target in targets if id(target) not in forced_ids
        ),
    )


def _filter_meson_source_group_to_existing(
    group: Mapping[str, Any],
    *,
    source_root: Path,
    build_root: Path,
) -> tuple[dict[str, Any], tuple[Path, ...]]:
    """Copy a linked static-lib source group keeping only on-disk source files.

    Generated sources of a linked static library may be transient build
    artifacts already cleaned from the build dir; those are dropped so a linked
    lib contributes exactly the translation units still present on disk. Non
    source-file keys (language/compiler/parameters/linker) are preserved so the
    compile-unit metadata for the surviving sources stays intact. Dropped
    generated sources are returned as source-plan diagnostics so a cleaned unit
    is explicit manifest evidence, never a silent producer-side skip.
    """
    filtered: dict[str, Any] = dict(group)
    skipped_generated_sources: list[Path] = []
    for key, prefer_build_root in (("sources", False), ("generated_sources", True)):
        raw_values = group.get(key)
        if not isinstance(raw_values, (list, tuple)):
            continue
        kept: list[Any] = []
        for raw_source in raw_values:
            source_path = _resolve_meson_plan_artifact_path(
                raw_source,
                source_root=source_root,
                build_root=build_root,
                prefer_build_root=prefer_build_root,
            )
            if source_path.exists():
                kept.append(raw_source)
            elif key == "generated_sources":
                skipped_generated_sources.append(source_path.resolve())
        filtered[key] = kept
    return filtered, _dedupe_paths(skipped_generated_sources)


def _load_meson_intro_targets_source_extension_plan(
    *,
    plan_path: Path,
    project_root: Path,
    module_name: str,
    selector: Any = None,
    source_root: Any = None,
    build_root: Any = None,
    compile_commands: Any = None,
    exclude_linked_static_libraries: Sequence[str] | None = None,
    python_provider: SourceExtensionPythonProvider | None = None,
) -> tuple[_SourceExtensionBuildPlan | None, list[str]]:
    errors: list[str] = []
    excluded_linked_static_libraries = tuple(exclude_linked_static_libraries or ())
    if not plan_path.exists() or not plan_path.is_file():
        return None, [f"source extension build plan not found: {plan_path}"]
    try:
        payload = json.loads(plan_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        return None, [f"failed to read Meson intro-targets plan {plan_path}: {exc}"]
    if not isinstance(payload, list):
        return None, [f"Meson intro-targets plan must be a JSON array: {plan_path}"]

    resolved_source_root = (
        _resolve_source_extension_plan_path(base=project_root, raw_path=source_root)
        if source_root is not None
        else project_root.resolve()
    )
    if build_root is not None:
        resolved_build_root = _resolve_source_extension_plan_path(
            base=project_root,
            raw_path=build_root,
        )
    elif plan_path.parent.name == "meson-info":
        resolved_build_root = plan_path.parent.parent.resolve()
    else:
        resolved_build_root = project_root.resolve()
    compile_commands_path = (
        _resolve_source_extension_plan_path(
            base=project_root,
            raw_path=compile_commands,
        )
        if compile_commands is not None
        else (resolved_build_root / "compile_commands.json").resolve()
    )

    selected = _source_extension_plan_target_selector(
        module_name=module_name,
        selector=selector,
    )
    try:
        matches = _meson_extension_targets_by_selector(payload).get(selected, ())
    except ValueError as exc:
        return None, [str(exc)]
    if not matches:
        return None, [f"Meson intro-targets plan has no target matching {selected!r}"]
    if len(matches) > 1:
        ids = ", ".join(str(match.get("id", "")) for match in matches)
        return None, [f"Meson intro-targets selector {selected!r} is ambiguous ({ids})"]

    target = matches[0]
    target_id = str(target.get("id", "")).strip()
    target_name = str(target.get("name", "")).strip()
    target_type = str(target.get("type", "")).strip()
    if not target_id:
        errors.append("Meson target is missing non-empty 'id'")
    if not target_name:
        errors.append("Meson target is missing non-empty 'name'")
    if target_type not in _MESON_EXTENSION_TARGET_TYPES:
        errors.append(
            f"Meson target {target_name or target_id!r} is not an extension module "
            f"target (type={target_type!r})"
        )
    target_sources = target.get("target_sources")
    if not isinstance(target_sources, list):
        errors.append(f"Meson target {target_name or target_id!r} lacks target_sources")
        target_sources = []

    # One metadata authority owns source folding, exclusions and remaining operands.
    try:
        projection = _meson_static_library_projection(
            primary_target=target,
            payload=payload,
            build_root=resolved_build_root,
            exclude_linked_static_libraries=excluded_linked_static_libraries,
        )
    except ValueError as exc:
        return None, [str(exc)]
    linked_static_targets = projection.targets
    link_args = projection.link_args
    provider_receipt = None
    if python_provider is not None:
        link_args, provider_receipt = python_provider.project(link_args)
    for linked_target in linked_static_targets:
        if str(linked_target["id"]) not in projection.forced_target_ids:
            continue
        for group in linked_target.get("target_sources") or ():
            if not isinstance(group, Mapping):
                continue
            for field, prefer_build in (
                ("sources", False),
                ("generated_sources", True),
            ):
                for raw_source in group.get(field) or ():
                    source_path = _resolve_meson_plan_artifact_path(
                        raw_source,
                        source_root=resolved_source_root,
                        build_root=resolved_build_root,
                        prefer_build_root=prefer_build,
                    )
                    if _is_compilable_source_path(source_path):
                        if not source_path.is_file():
                            errors.append(
                                "Meson forced static-library member source is missing: "
                                + str(source_path)
                            )
    owned_source_groups: list[tuple[str, Mapping[str, Any]]] = [
        (target_id, group) for group in target_sources if isinstance(group, Mapping)
    ]
    skipped_generated_sources: list[Path] = []
    for linked_target in linked_static_targets:
        linked_sources = linked_target.get("target_sources")
        if not isinstance(linked_sources, list):
            continue
        for group in linked_sources:
            if not isinstance(group, Mapping):
                continue
            # Static-library groups can list generated sources whose on-disk
            # ``.c`` was a transient build artifact that has since been cleaned
            # (numpy's tempita ``.c.src`` templates). The primary target already
            # links what it needs; a linked-lib source is admitted ONLY when its
            # file still resolves, so a cleaned generated unit is skipped rather
            # than hard-failing the whole plan. Sources present on disk (the
            # linked-in symbols the reachability demands, e.g. unique.cpp) are
            # included.
            filtered_group, skipped = _filter_meson_source_group_to_existing(
                group,
                source_root=resolved_source_root,
                build_root=resolved_build_root,
            )
            owned_source_groups.append((str(linked_target["id"]), filtered_group))
            skipped_generated_sources.extend(skipped)

    object_roots_by_owner = {
        str(owner["id"]): _meson_target_object_roots(
            owner.get("filename"), build_root=resolved_build_root
        )
        for owner in (target, *linked_static_targets)
    }
    for owner_id, roots in object_roots_by_owner.items():
        if not roots or any(
            not _path_is_within(root, resolved_build_root) for root in roots
        ):
            errors.append(f"Meson target {owner_id!r} has no build-owned object roots")
    if errors:
        return None, errors
    target_output_roots = _dedupe_paths(
        [root for roots in object_roots_by_owner.values() for root in roots]
    )

    target_compile_command_sources: list[Path] = []
    for _owner_id, source_group in owned_source_groups:
        for raw_source in source_group.get("sources") or ():
            source_path = _resolve_meson_plan_artifact_path(
                raw_source,
                source_root=resolved_source_root,
                build_root=resolved_build_root,
                prefer_build_root=False,
            )
            if _is_compilable_source_path(source_path):
                target_compile_command_sources.append(source_path.resolve())
        for raw_source in source_group.get("generated_sources") or ():
            source_path = _resolve_meson_plan_artifact_path(
                raw_source,
                source_root=resolved_source_root,
                build_root=resolved_build_root,
                prefer_build_root=True,
            )
            if _is_compilable_source_path(source_path):
                target_compile_command_sources.append(source_path.resolve())
    compile_command_units, compile_command_errors = _load_compile_command_units(
        compile_commands_path,
        required_sources=set(_dedupe_paths(target_compile_command_sources)),
        target_output_roots=target_output_roots,
    )
    errors.extend(compile_command_errors)
    if compile_command_units is None:
        return None, errors

    commands_by_owner_source: dict[tuple[str, Path], list[_CompileCommandUnit]] = {}
    for command_unit in compile_command_units.values():
        owners = [
            owner_id
            for owner_id, roots in object_roots_by_owner.items()
            if any(_path_is_within(command_unit.object_path, root) for root in roots)
        ]
        if len(owners) != 1:
            errors.append(
                "Meson compile output must have exactly one target owner: "
                f"{command_unit.object_path} ({owners!r})"
            )
            continue
        commands_by_owner_source.setdefault(
            (owners[0], command_unit.source_path), []
        ).append(command_unit)

    sources: list[Path] = []
    generated_sources: list[Path] = []
    non_compiled_inputs: list[Path] = []
    compile_units: list[_SourceExtensionCompileUnit] = []
    compile_units_by_object: dict[Path, _SourceExtensionCompileUnit] = {}
    include_dirs: list[Path] = []
    compile_args: list[str] = []

    def append_compile_unit(
        *,
        source_path: Path,
        owner_id: str,
        generated: bool,
        language: str | None,
        command_unit: _CompileCommandUnit,
    ) -> None:
        resolved_source_path = source_path.resolve()
        try:
            resolved_language, unit_args = resolve_source_extension_compile_language(
                source_path=resolved_source_path,
                language=language,
                compile_args=command_unit.compile_args,
            )
        except ValueError as exc:
            errors.append(f"invalid compile language for {resolved_source_path}: {exc}")
            return
        unit = _SourceExtensionCompileUnit(
            source_path=resolved_source_path,
            owner_target_id=owner_id,
            producer_object_path=command_unit.object_path,
            generated=generated,
            language=resolved_language,
            compiler=command_unit.compiler,
            include_dirs=command_unit.include_dirs,
            compile_args=unit_args,
            force_include=owner_id in projection.forced_target_ids,
        )
        existing = compile_units_by_object.get(command_unit.object_path)
        if existing is not None:
            if existing != unit:
                errors.append(
                    "Meson object output has conflicting compile-unit metadata: "
                    f"{command_unit.object_path}"
                )
            return
        compile_units_by_object[command_unit.object_path] = unit
        compile_units.append(unit)
        compile_args.extend(unit_args)
        include_dirs.extend(command_unit.include_dirs)

    for owner_id, source_group in owned_source_groups:
        language_value = source_group.get("language")
        language = (
            language_value.strip()
            if isinstance(language_value, str) and language_value.strip()
            else None
        )
        for field, generated, destination in (
            ("sources", False, sources),
            ("generated_sources", True, generated_sources),
        ):
            for raw_source in source_group.get(field) or ():
                source_path = _resolve_meson_plan_artifact_path(
                    raw_source,
                    source_root=resolved_source_root,
                    build_root=resolved_build_root,
                    prefer_build_root=generated,
                )
                if not _is_compilable_source_path(source_path):
                    non_compiled_inputs.append(source_path)
                    continue
                destination.append(source_path)
                command_units = commands_by_owner_source.get(
                    (owner_id, source_path.resolve()), ()
                )
                if not command_units:
                    errors.append(
                        "compile_commands.json has no owned entry for Meson target "
                        f"{owner_id!r} source: {source_path.resolve()}"
                    )
                for command_unit in command_units:
                    append_compile_unit(
                        source_path=source_path,
                        owner_id=owner_id,
                        generated=generated,
                        language=language,
                        command_unit=command_unit,
                    )

    deduped_sources = _dedupe_paths(sources)
    deduped_generated_sources = _dedupe_paths(generated_sources)
    deduped_skipped_generated_sources = _dedupe_paths(skipped_generated_sources)
    all_sources = (*deduped_sources, *deduped_generated_sources)
    if not all_sources:
        errors.append(
            f"Meson target {target_name or target_id!r} does not expose any "
            "compiled C/C++/Objective-C source units"
        )
    deduped_non_compiled_inputs = _dedupe_paths(non_compiled_inputs)
    existing_pyx_inputs = tuple(
        source_path
        for source_path in deduped_non_compiled_inputs
        if source_path.suffix.lower() == ".pyx" and source_path.is_file()
    )
    ninja_proven_pyx_inputs: list[Path] = []
    regenerable_generated_sources: set[Path] = set()
    for source_path in deduped_generated_sources:
        if source_path.is_file():
            continue
        pyx_matches = _source_extension_cython.generated_c_pyx_matches(
            generated_c=source_path,
            pyx_candidates=existing_pyx_inputs,
        )
        if not pyx_matches:
            ninja_pyx, ninja_error = (
                _source_extension_cython.generated_c_pyx_from_ninja(
                    generated_c=source_path,
                    build_root=resolved_build_root,
                )
            )
            if ninja_error is not None:
                errors.append(ninja_error)
                continue
            if ninja_pyx is not None:
                pyx_matches = (ninja_pyx,)
                ninja_proven_pyx_inputs.append(ninja_pyx)
        if len(pyx_matches) == 1:
            # Keep the real compile_commands row for the absent output. The
            # extension-build consumer replaces this unit's source path with
            # standalone Cython output before invoking the compiler; no fake
            # or placeholder C file is materialized by plan loading.
            regenerable_generated_sources.add(source_path.resolve())
            continue
        if len(pyx_matches) > 1:
            errors.append(
                "Meson target generated source has ambiguous same-stem Cython "
                f"inputs: {source_path} -> "
                + ", ".join(str(path) for path in pyx_matches)
            )
            continue
        errors.append(
            "Meson target generated source does not exist and has no unique "
            f"target-local or Ninja-proven .pyx input: {source_path}"
        )
    deduped_non_compiled_inputs = _dedupe_paths(
        (*deduped_non_compiled_inputs, *ninja_proven_pyx_inputs)
    )
    for source_path in all_sources:
        if source_path.resolve() in regenerable_generated_sources:
            continue
        if not source_path.exists() or not source_path.is_file():
            errors.append(f"Meson target source does not exist: {source_path}")
    for source_path in deduped_non_compiled_inputs:
        if not source_path.exists() or not source_path.is_file():
            errors.append(f"Meson target input does not exist: {source_path}")

    if errors:
        if deduped_non_compiled_inputs:
            errors.append(
                "Meson target non-compiled inputs: "
                + ", ".join(str(path) for path in deduped_non_compiled_inputs[:8])
            )
        return None, errors

    plan = _SourceExtensionBuildPlan(
        kind="meson-intro-targets",
        plan_path=plan_path.resolve(),
        plan_sha256=_sha256_file(plan_path),
        compile_commands_path=compile_commands_path,
        compile_commands_sha256=_sha256_file(compile_commands_path),
        target_id=target_id,
        target_name=target_name,
        target_selector=selected,
        target_type=target_type,
        source_root=resolved_source_root,
        build_root=resolved_build_root,
        sources=deduped_sources,
        generated_sources=deduped_generated_sources,
        skipped_generated_sources=deduped_skipped_generated_sources,
        non_compiled_inputs=deduped_non_compiled_inputs,
        compile_units=tuple(compile_units),
        include_dirs=_dedupe_paths(include_dirs),
        compile_args=tuple(compile_args),
        link_args=link_args,
        consumed_forced_link_args=projection.consumed_forced_args,
        producer_link_args=projection.producer_link_args,
        lazy_static_target_ids=projection.lazy_static_target_ids,
        dependencies_path=python_provider.dependencies_path
        if python_provider
        else None,
        dependencies_sha256=python_provider.dependencies_sha256
        if python_provider
        else None,
        python_provider=provider_receipt,
        digest="",
    )
    return (
        replace(plan, digest=_source_extension_build_plan_digest(plan)),
        [],
    )


def _coerce_plan_string_sequence(value: Any) -> tuple[str, ...]:
    """Normalize a source-plan string/list option to a tuple of strings.

    Accepts a single string (one entry), a list/tuple of strings, or ``None``
    (no entries). Non-string members and blanks are dropped so a malformed
    config degrades to an empty selector rather than raising.
    """
    if value is None:
        return ()
    if isinstance(value, str):
        stripped = value.strip()
        return (stripped,) if stripped else ()
    if isinstance(value, (list, tuple)):
        return tuple(
            item.strip() for item in value if isinstance(item, str) and item.strip()
        )
    return ()


def _load_source_extension_build_plan(
    *,
    project_root: Path,
    module_name: str,
    plan_config: Mapping[str, Any],
    python_provider: SourceExtensionPythonProvider | None = None,
) -> tuple[_SourceExtensionBuildPlan | None, list[str]]:
    kind = plan_config.get("kind") or plan_config.get("type") or "meson-intro-targets"
    if not isinstance(kind, str) or kind not in _SOURCE_EXTENSION_PLAN_KINDS:
        return None, [
            "tool.molt.extension.source_plan.kind must be one of "
            f"{sorted(_SOURCE_EXTENSION_PLAN_KINDS)}"
        ]
    raw_plan_path = plan_config.get("path") or plan_config.get("intro_targets")
    if not isinstance(raw_plan_path, str) or not raw_plan_path.strip():
        return None, [
            "tool.molt.extension.source_plan.path must point at a Meson "
            "intro-targets.json file"
        ]
    plan_path = _resolve_source_extension_plan_path(
        base=project_root,
        raw_path=raw_plan_path,
    )
    if kind == "meson-intro-targets":
        return _load_meson_intro_targets_source_extension_plan(
            plan_path=plan_path,
            project_root=project_root,
            module_name=module_name,
            selector=plan_config.get("target")
            or plan_config.get("target_id")
            or plan_config.get("target_name"),
            source_root=plan_config.get("source_root")
            or plan_config.get("source-root"),
            build_root=plan_config.get("build_root") or plan_config.get("build-root"),
            compile_commands=plan_config.get("compile_commands")
            or plan_config.get("compile-commands")
            or plan_config.get("compile_commands_path")
            or plan_config.get("compile-commands-path"),
            exclude_linked_static_libraries=_coerce_plan_string_sequence(
                plan_config.get("exclude_linked_static_libraries")
                or plan_config.get("exclude-linked-static-libraries")
            ),
            python_provider=python_provider,
        )
    return None, [f"unsupported source extension build plan kind: {kind!r}"]


def _validate_source_extension_build_plan_target(
    plan: _SourceExtensionBuildPlan,
    *,
    target_triple: str,
) -> list[str]:
    require_explicit = source_extension_target_is_wasm(target_triple)
    errors: list[str] = []
    dialect = source_extension_link_dialect(target_triple)
    try:
        for span in source_extension_link_arguments(plan.producer_link_args):
            span.validate_dialect(dialect)
        for span in source_extension_link_arguments(plan.consumed_forced_link_args):
            span.validate_dialect(dialect)
            if span.kind != "forced":
                raise ValueError(
                    f"invalid consumed forced loading operand: {span.arguments!r}"
                )
    except ValueError as exc:
        errors.append(f"Source-plan linker custody: {exc}")
    for unit in plan.compile_units:
        try:
            explicit = validate_compiler_target(
                (*unit.compiler, *unit.compile_args),
                compiler_target_triple(unit.compiler, target_triple),
            )
            if require_explicit and not explicit:
                raise ValueError(
                    "WASM source-extension builds require a target-specific upstream "
                    f"compile_commands.json with an explicit {target_triple} target"
                )
        except ValueError as exc:
            errors.append(f"Source-plan target custody for {unit.source_path}: {exc}")
    return errors


def _source_extension_gc_compile_args(*, target_triple: str | None) -> list[str]:
    target = (target_triple or "").lower()
    if "windows-msvc" in target or (not target and os.name == "nt"):
        return []
    return ["-ffunction-sections", "-fdata-sections"]


def _source_extension_wasm_compile_args(
    *,
    target_triple: str | None,
    cc_cmd: Sequence[str],
) -> list[str]:
    target = (target_triple or "").lower()
    if not target.startswith("wasm32") or not cc_cmd:
        return []
    tool = Path(cc_cmd[0]).name.lower()
    if tool in {"zig", "zig.exe"} or "clang" in tool:
        return [
            "-mexception-handling",
            "-mllvm",
            "-wasm-enable-sjlj",
            "-fno-fast-math",
            "-ffp-contract=off",
        ]
    return []


def _source_extension_replay_compile_args(
    unit_compile_args: Sequence[str],
    *,
    compiler_target: str,
    compiler_command: Sequence[str] = (),
) -> list[str]:
    """Replay semantic spans without duplicating target or operand authority."""
    validate_compiler_target(unit_compile_args, compiler_target)
    dialect = source_extension_compiler_dialect(compiler_command or ("clang",))
    out: list[str] = []
    joined_options = tuple(option + "=" for option in COMPILER_OWNED_OPTIONS) + (
        "/winsysroot:",
        "--driver-mode=",
        "-ffile-prefix-map=",
        "-fdebug-prefix-map=",
        "-fmacro-prefix-map=",
        "/pathmap:",
    )
    for span in compiler_argument_spans(unit_compile_args):
        token = span.option
        if span.context in {"driver", "cc1"}:
            _reject_unowned_precompiled_input(token)
            if (
                token in COMPILER_OWNED_OPTIONS
                or token.startswith(joined_options)
                or token in {"-m32", "-m64", "-mx32"}
                or span.is_output
            ):
                continue
        # The complete span crosses the driver transport boundary as one unit.
        # In particular an LLVM/assembler/preprocessor operand is never parsed
        # again as an output, language, target, macro or include selector.
        forward = token.startswith("-") and span.context != "positional"
        out.extend(
            dialect.forward(value) if forward else value for value in span.arguments
        )
    return out


def _source_extension_object_fact(
    *,
    source_path: Path,
    object_path: Path,
    language: SourceExtensionLanguage,
    compile_command: Sequence[str] = (),
    dependency_paths: Sequence[Path] = (),
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    producer_unit: _SourceExtensionCompileUnitIdentity | None = None,
) -> tuple[_SourceExtensionObjectFact | None, str | None]:
    from molt.cli.native_symbol_inspection import (
        NativeSymbolInspectionError,
    )

    try:
        symbol_inspection = _inspect_source_extension_artifact_symbols(
            object_path,
            nm_command=nm_command,
            target_triple=target_triple,
        )
    except NativeSymbolInspectionError as error:
        return None, str(error)
    if symbol_inspection is None:
        return (
            None,
            "unable to read global symbol table for compiled extension object "
            f"{object_path}; canonical symbol authority is unavailable",
        )
    defined = symbol_inspection.defined_symbols
    undefined = symbol_inspection.undefined_symbols
    symbol_authority = symbol_inspection.symbol_authority
    object_root = object_path.parent.resolve()
    canonical_compile_command_parts: list[str] = []
    for span in compiler_argument_spans(compile_command):
        transient = span.context == "driver" and (
            span.output_option in {"-o", "-MF", "/Fo"}
            or span.option.startswith(
                (
                    "-ffile-prefix-map=",
                    "-fdebug-prefix-map=",
                    "-fmacro-prefix-map=",
                    "/pathmap:",
                )
            )
        )
        for token in span.raw:
            canonical = token
            if transient:
                canonical = token.replace(str(object_root), "@object-root").replace(
                    object_root.as_posix(), "@object-root"
                )
                if "@object-root" in canonical:
                    canonical = canonical.replace("\\", "/")
            canonical_compile_command_parts.append(canonical)
    canonical_compile_command = tuple(canonical_compile_command_parts)
    dependencies: list[_SourceExtensionDependencyFact] = []
    for dependency in sorted({path.resolve() for path in dependency_paths}):
        if dependency == source_path.resolve():
            continue
        if not dependency.is_file():
            return None, f"compiled extension dependency is missing: {dependency}"
        dependencies.append(
            _SourceExtensionDependencyFact(
                path=dependency,
                sha256=_sha256_file(dependency),
            )
        )
    dependencies.sort(
        key=lambda fact: (fact.sha256, fact.path.name, fact.path.as_posix())
    )
    inspected_digest = symbol_inspection.artifact_digest
    try:
        if inspected_digest is None or _sha256_file(object_path) != inspected_digest:
            return (
                None,
                f"compiled extension object changed after symbol inspection: {object_path}",
            )
    except OSError as error:
        return None, f"cannot verify inspected extension object {object_path}: {error}"
    return (
        _SourceExtensionObjectFact(
            source_path=source_path.resolve(),
            language=language,
            object_path=object_path,
            source_sha256=_sha256_file(source_path),
            object_sha256=inspected_digest,
            defined_symbols=tuple(sorted(defined)),
            undefined_symbols=tuple(sorted(undefined)),
            defined_function_symbols=tuple(
                sorted(symbol_inspection.defined_function_symbols)
            ),
            compile_command=canonical_compile_command,
            symbol_authority=symbol_authority,
            symbol_command=(
                tuple(nm_command or ())
                if symbol_authority == SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY
                else ()
            ),
            dependencies=tuple(dependencies),
            producer_unit=producer_unit,
        ),
        None,
    )


def _inspect_source_extension_artifact_symbols(
    artifact_path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    aggregate_linker_closure: bool = False,
) -> _SourceExtensionArtifactSymbolInspection | None:
    try:
        with artifact_path.open("rb") as stream:
            header = stream.read(8)
            is_wasm = header == b"\0asm\x01\0\0\0"
            artifact_bytes = header + stream.read() if is_wasm else None
    except OSError:
        return None
    if is_wasm:
        from molt.wasm_artifact import parse_wasm_relocatable_object_interface

        try:
            assert artifact_bytes is not None
            interface = parse_wasm_relocatable_object_interface(
                artifact_bytes,
                signature_import_names=None,
            )
        except (OSError, UnicodeDecodeError, ValueError, IndexError):
            return None
        defined = set(interface.linking_symbols.defined_names)
        undefined = set(interface.linking_symbols.undefined_names)
        if aggregate_linker_closure:
            undefined.difference_update(defined)
        return _SourceExtensionArtifactSymbolInspection(
            defined_symbols=frozenset(defined),
            undefined_symbols=frozenset(undefined),
            defined_function_symbols=interface.linking_symbols.defined_functions,
            symbol_authority=SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
            wasm_imports=interface.imports,
            wasm_function_import_signatures=interface.function_import_signatures,
            wasm_function_exports=tuple(
                sorted(export.name for export in interface.function_exports)
            ),
            artifact_bytes=artifact_bytes,
            artifact_digest=hashlib.sha256(artifact_bytes).hexdigest(),
            wasm_interface=interface,
        )

    # Reading a native object file's global symbols is a backend/native-link
    # concern; import it lazily so this module stays off the frontend import path.
    from molt.cli.native_symbol_inspection import (
        _native_archive_global_symbol_facts,
        _native_object_global_symbol_facts,
    )

    if not nm_command and not aggregate_linker_closure:
        return None
    symbol_facts = (
        _native_archive_global_symbol_facts(artifact_path, target_triple=target_triple)
        if not nm_command
        else _native_object_global_symbol_facts(
            artifact_path,
            nm_command=nm_command,
            target_triple=target_triple,
        )
    )
    defined = set(symbol_facts.defined)
    undefined = set(symbol_facts.undefined)
    if aggregate_linker_closure:
        undefined.difference_update(defined)
    return _SourceExtensionArtifactSymbolInspection(
        defined_symbols=frozenset(defined),
        undefined_symbols=frozenset(undefined),
        defined_function_symbols=symbol_facts.defined_functions,
        symbol_authority=SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
        wasm_imports=None,
        wasm_function_import_signatures=(),
        artifact_digest=symbol_facts.artifact_digest,
    )


def validate_source_extension_artifact_object_closure(
    *,
    artifact_path: Path,
    manifest: Mapping[str, Any],
    required_function_exports: Collection[str] = (),
    inspection: _SourceExtensionArtifactSymbolInspection | None = None,
    external_link_provider_classes: Mapping[str, str] | None = None,
    package_function_signatures: Mapping[str, tuple[tuple[str, ...], str]]
    | None = None,
    validated_closure: tuple[Mapping[str, Any], str] | None = None,
) -> list[str]:
    """Validate the signed closure against one exact artifact inspection."""

    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ["extension manifest has no object_closure"]
    try:
        identity, _closure_digest = (
            validate_source_extension_object_closure(manifest)
            if validated_closure is None
            else validated_closure
        )
    except SourceExtensionObjectClosureError as exc:
        return [f"object_closure is invalid: {exc}"]
    artifact_kind = manifest.get("artifact_kind")
    target_triple = manifest.get("target_triple")
    if not isinstance(target_triple, str) or not target_triple:
        return ["extension manifest has no target_triple"]

    if artifact_kind == "wasm_relocatable_object":
        if inspection is None:
            inspection = _inspect_source_extension_artifact_symbols(
                artifact_path,
                target_triple=target_triple,
                aggregate_linker_closure=True,
            )
        if inspection is None or inspection.wasm_imports is None:
            return [
                f"cannot read WASM linking/import closure from {artifact_path.name}"
            ]
        if inspection.artifact_bytes is None or hashlib.sha256(
            inspection.artifact_bytes
        ).hexdigest() != manifest.get("extension_sha256"):
            return [
                f"WASM inspected bytes differ from extension_sha256: {artifact_path.name}"
            ]
        actual_defined = set(inspection.defined_symbols)
        actual_undefined = set(inspection.undefined_symbols)
        actual_defined_functions = set(inspection.defined_function_symbols)
    elif artifact_kind == "static_archive":
        if inspection is not None:
            if (
                inspection.artifact_digest is None
                or inspection.artifact_digest != manifest.get("extension_sha256")
            ):
                return [
                    f"Native inspected bytes differ from extension_sha256: {artifact_path.name}"
                ]
            actual_defined = set(inspection.defined_symbols)
            actual_undefined = set(inspection.undefined_symbols) - actual_defined
            actual_defined_functions = set(inspection.defined_function_symbols)
        else:
            from molt.cli.native_symbol_inspection import (
                NativeSymbolInspectionError,
                _native_archive_global_symbol_facts,
            )

            try:
                symbol_facts = _native_archive_global_symbol_facts(
                    artifact_path,
                    target_triple=target_triple,
                )
            except NativeSymbolInspectionError as error:
                return [str(error)]
            if (
                symbol_facts.artifact_digest is None
                or symbol_facts.artifact_digest != manifest.get("extension_sha256")
            ):
                return [
                    f"Native inspected bytes differ from extension_sha256: {artifact_path.name}"
                ]
            actual_defined = set(symbol_facts.defined)
            actual_undefined = set(symbol_facts.undefined) - actual_defined
            actual_defined_functions = set(symbol_facts.defined_functions)
    else:
        return [f"unsupported source-extension artifact_kind {artifact_kind!r}"]

    declared_defined = set(identity["defined_symbols"])
    declared_undefined = set(identity["undefined_symbols"])
    errors: list[str] = []
    if declared_defined != actual_defined:
        errors.append(
            f"{artifact_path.name} defined-symbol closure differs from "
            "object_closure.defined_symbols; "
            f"missing={sorted(actual_defined - declared_defined)!r}; "
            f"stale={sorted(declared_defined - actual_defined)!r}"
        )
    if declared_undefined != actual_undefined:
        errors.append(
            f"{artifact_path.name} undefined-symbol closure differs from "
            "object_closure.undefined_symbols; "
            f"missing={sorted(actual_undefined - declared_undefined)!r}; "
            f"stale={sorted(declared_undefined - actual_undefined)!r}"
        )

    root_symbol = identity["root_symbol"]
    if root_symbol not in actual_defined_functions:
        errors.append(
            f"object_closure.root_symbol {root_symbol!r} is not a defined "
            f"function in {artifact_path.name}"
        )

    required_exports = set(required_function_exports)
    available_function_exports = (
        set(inspection.wasm_function_exports)
        if artifact_kind == "wasm_relocatable_object" and inspection is not None
        else actual_defined_functions
    )
    missing_exports = sorted(required_exports - available_function_exports)
    if missing_exports:
        errors.append(
            "direct_symbol callable export(s) are absent from artifact function "
            "exports: " + ", ".join(missing_exports)
        )

    if artifact_kind == "static_archive":
        declared_runtime_symbols = set(identity["runtime_symbols"])
        if declared_runtime_symbols:
            errors.append(
                "native object_closure.runtime_symbols must be empty; native "
                "providers are owned by typed link requirements"
            )
        return errors
    if inspection is None:
        return errors
    try:
        actual_receipts = source_extension_wasm_import_receipts(
            inspection.wasm_imports or ()
        )
    except SourceExtensionObjectClosureError as exc:
        errors.append(f"WASM import closure is invalid: {exc}")
        return errors
    expected_runtime_symbols = set(
        wasm_static_link_runtime_symbols_for_imports(
            (*actual_undefined, *(item["name"] for item in actual_receipts)),
            typed_imports=((item["module"], item["name"]) for item in actual_receipts),
        )
    )
    declared_runtime_symbols = set(identity["runtime_symbols"])
    if declared_runtime_symbols != expected_runtime_symbols:
        errors.append(
            "object_closure.runtime_symbols differs from the exact generated "
            "runtime projection; "
            f"missing={sorted(expected_runtime_symbols - declared_runtime_symbols)!r}; "
            f"stale={sorted(declared_runtime_symbols - expected_runtime_symbols)!r}"
        )
    declared_receipts = tuple(identity.get("wasm_imports", ()))
    if declared_receipts != actual_receipts:
        declared_set = {
            (item["module"], item["name"], item["kind"]) for item in declared_receipts
        }
        actual_set = {
            (item["module"], item["name"], item["kind"]) for item in actual_receipts
        }
        missing = sorted(actual_set - declared_set)
        stale = sorted(declared_set - actual_set)
        if missing:
            errors.append(
                f"{artifact_path.name} import records absent from "
                f"object_closure.wasm_imports: {missing!r}"
            )
        if stale:
            errors.append(
                "object_closure.wasm_imports records absent from "
                f"{artifact_path.name}: {stale!r}"
            )
    errors.extend(
        validate_source_extension_wasm_import_shapes(
            actual_receipts,
            provider_function_signatures=package_function_signatures,
            provider_function_names=(
                (
                    wasm_external_link_provider_symbol_classes(target_triple)
                    if external_link_provider_classes is None
                    else external_link_provider_classes
                )
                if any(
                    receipt["name"] not in WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES
                    for receipt in actual_receipts
                )
                else ()
            ),
            actual_function_signatures={
                (module, name): (params, result)
                for module, name, params, result in (
                    inspection.wasm_function_import_signatures
                )
            },
        )
    )
    return errors


def source_extension_required_capsule_imports(
    source_text: str,
) -> dict[str, tuple[str, ...]]:
    sanitized = _strip_c_like_comments_and_literals(source_text)
    tokens = {match.group(0) for match in _C_IDENTIFIER_RE.finditer(sanitized)}
    imports_by_capsule: dict[str, list[str]] = {}
    for token, capsule in _SOURCE_EXTENSION_CAPSULE_IMPORT_TOKENS.items():
        if token in tokens:
            imports_by_capsule.setdefault(capsule, []).append(token)
    return {
        capsule: tuple(sorted(import_tokens))
        for capsule, import_tokens in sorted(imports_by_capsule.items())
    }


def _extract_source_extension_required_capsules(source_text: str) -> tuple[str, ...]:
    return tuple(source_extension_required_capsule_imports(source_text))


def source_extension_manifest_path(raw_path: str, *, manifest_path: Path) -> Path:
    source_path = Path(raw_path).expanduser()
    if not source_path.is_absolute():
        source_path = manifest_path.parent / source_path
    return source_path.resolve()


def _source_extension_missing_source_error(field_name: str, path: Path) -> str:
    return f"{_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX} {field_name}: {path}"


def source_extension_manifest_errors_are_missing_sources(
    errors: Sequence[str],
) -> bool:
    return bool(errors) and all(
        error.startswith(_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX)
        for error in errors
    )


def _scan_source_extension_manifest_inputs(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
    scanner: Callable[[str], _SourceScanResult],
    allow_missing_sources: bool = False,
) -> tuple[dict[Path, _SourceScanResult] | None, list[str]]:
    """Resolve the validated object-input authority, including dependencies.

    Capsule consumers may explicitly request a partial scan with missing-input
    diagnostics. Structural or checksum errors always invalidate the scan;
    eager Python import canonicalization never allows partial source custody.
    """
    try:
        rows = source_extension_manifest_input_rows(manifest)
    except SourceExtensionInputCustodyError as exc:
        return None, [str(exc)]
    errors: list[str] = []
    results: dict[Path, _SourceScanResult] = {}
    inspected: set[tuple[str, str]] = set()
    for field, raw_path, digest in rows:
        if (raw_path, digest) in inspected:
            continue
        inspected.add((raw_path, digest))
        source_path, content, source_errors = read_source_extension_manifest_input(
            raw_path,
            manifest_path=manifest_path,
            expected_sha256=digest,
        )
        errors.extend(source_errors)
        if source_path is not None:
            assert content is not None
            if source_path not in results:
                results[source_path] = scanner(
                    content.decode("utf-8", errors="replace")
                )
        elif not source_errors:
            errors.append(
                _source_extension_missing_source_error(
                    field,
                    source_extension_manifest_path(
                        raw_path, manifest_path=manifest_path
                    ),
                )
            )
    if errors and not (
        allow_missing_sources
        and source_extension_manifest_errors_are_missing_sources(errors)
    ):
        return None, errors
    return results, errors


def source_extension_manifest_required_capsule_imports_by_source(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
    allow_missing_sources: bool = False,
) -> tuple[dict[Path, dict[str, tuple[str, ...]]] | None, list[str]]:
    """Map each resolvable manifest source to its required capsule imports.

    ``allow_missing_sources`` explicitly permits capsule consumers to scan the
    resolvable subset while retaining missing-input diagnostics. It never relaxes
    structural input validation or checksum custody.
    """
    by_source, source_errors = _scan_source_extension_manifest_inputs(
        manifest,
        manifest_path=manifest_path,
        scanner=source_extension_required_capsule_imports,
        allow_missing_sources=allow_missing_sources,
    )
    non_missing_errors = [
        error
        for error in source_errors
        if not error.startswith(_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX)
    ]
    if non_missing_errors:
        return None, source_errors
    if by_source is None:
        return None, source_errors
    missing_diagnostics = [
        error
        for error in source_errors
        if error.startswith(_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX)
    ]
    return {
        source: required for source, required in by_source.items() if required
    }, missing_diagnostics


def canonicalize_source_extension_manifest_required_capsules(
    manifest: dict[str, Any],
    *,
    manifest_path: Path,
    allow_missing_sources: bool = False,
) -> list[str]:
    """Persist source-derived capsule custody into the closure and its objects."""
    try:
        projected = _expand_source_extension_manifest_authorities(manifest)
    except ValueError as exc:
        return [str(exc)]
    by_source, errors = source_extension_manifest_required_capsule_imports_by_source(
        projected,
        manifest_path=manifest_path,
        allow_missing_sources=allow_missing_sources,
    )
    if errors and not (
        allow_missing_sources
        and source_extension_manifest_errors_are_missing_sources(errors)
    ):
        return errors
    if by_source is None:
        return errors
    if not by_source:
        return errors
    object_closure = projected.get("object_closure")
    if not isinstance(object_closure, dict):
        return ["source-extension manifest requires non-empty object_closure custody"]

    def string_set(value: object) -> set[str]:
        if not isinstance(value, list):
            return set()
        return {
            item.strip() for item in value if isinstance(item, str) and item.strip()
        }

    required_capsules = string_set(object_closure.get("required_capsules"))
    for imports_by_capsule in by_source.values():
        required_capsules.update(imports_by_capsule)
    objects = object_closure.get("objects")
    if not isinstance(objects, list):
        return ["source-extension manifest requires non-empty object_closure.objects"]
    updates: list[tuple[dict[str, Any], list[str]]] = []
    for item in objects:
        if not isinstance(item, dict):
            return ["source-extension manifest object must be mutable"]
        source = item.get("source")
        if not isinstance(source, str) or not source.strip():
            continue
        item_capsules = string_set(item.get("required_capsules"))
        inputs = [source]
        if "dependencies" in item:
            inputs.extend(
                dependency["path"]
                for dependency in _manifest_dependencies(projected, item)
            )
        for raw_input in inputs:
            source_path = source_extension_manifest_path(
                raw_input, manifest_path=manifest_path
            )
            item_capsules.update(by_source.get(source_path, {}))
        updates.append((item, sorted(item_capsules)))
    object_closure["required_capsules"] = sorted(required_capsules)
    for item, capsules in updates:
        item["required_capsules"] = capsules
    manifest.clear()
    manifest.update(projected)
    return errors


def canonicalize_source_extension_manifest_runtime_python_imports(
    manifest: dict[str, Any],
    *,
    manifest_path: Path,
) -> list[str]:
    """Persist known eager import facts only after scanning every owned input."""
    scanned, errors = source_extension_manifest_runtime_python_imports(
        manifest, manifest_path=manifest_path
    )
    if errors:
        return errors
    try:
        manifest["runtime_python_import_modules"] = encode_python_module_names(
            scanned, field="runtime_python_import_modules"
        )
    except ValueError as exc:
        return [str(exc)]
    return []


def source_extension_manifest_runtime_python_imports(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
) -> tuple[tuple[str, ...], list[str]]:
    """Derive known eager C imports from the complete owned source closure.

    Missing, unreadable or corrupt inputs never become an empty/partial seal.
    Nonliteral runtime import names retain the existing dynamic execution policy;
    scanning literal roots is not proof of arbitrary dynamic import completeness.
    """
    scanned, errors = _scan_source_extension_manifest_inputs(
        manifest,
        manifest_path=manifest_path,
        scanner=source_extension_runtime_python_imports,
        allow_missing_sources=False,
    )
    if scanned is None:
        return (), errors
    names = {name for imports in scanned.values() for name in imports}
    return tuple(sorted(names)), errors


def source_extension_manifest_required_capsule_imports(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
) -> tuple[dict[str, tuple[str, ...]] | None, list[str]]:
    by_source, errors = source_extension_manifest_required_capsule_imports_by_source(
        manifest,
        manifest_path=manifest_path,
    )
    if errors:
        return None, errors
    assert by_source is not None
    by_capsule: dict[str, set[str]] = {}
    for imports_by_capsule in by_source.values():
        for capsule, import_tokens in imports_by_capsule.items():
            by_capsule.setdefault(capsule, set()).update(import_tokens)
    return {
        capsule: tuple(sorted(import_tokens))
        for capsule, import_tokens in sorted(by_capsule.items())
    }, []


def _source_extension_required_c_api_by_object(
    *,
    molt_root: Path,
    object_facts: Sequence[_SourceExtensionObjectFact],
    python_header: Path | None = None,
) -> tuple[_SourceExtensionCAPIRequirements | None, str | None]:
    """Classify the retained compiled closure, never a textual approximation.

    Source and compiler-attested dependencies provide only context-independent
    capsule and generated-prefix metadata. Their bytes may be shared; compiled
    undefined symbols and dependency membership always belong to an object.
    """
    objects_by_path: dict[Path, _SourceExtensionObjectFact] = {}
    for fact in object_facts:
        object_path = fact.object_path.resolve()
        if object_path in objects_by_path:
            return (
                None,
                f"source extension C/API scan has duplicate object: {object_path}",
            )
        objects_by_path[object_path] = fact

    scan_surface, header_path, header_error = _load_c_api_scan_surface(
        molt_root,
        header_path=python_header,
    )
    if header_error is not None:
        return (
            None,
            f"failed to read libmolt Python.h surface ({header_path}): {header_error}",
        )
    assert scan_surface is not None

    project_defined_symbols = {
        symbol for fact in object_facts for symbol in fact.defined_symbols
    }
    # Metadata extraction is independent of compiler flags, so bytes and their
    # extracted metadata are cached once. Checksums still bind every reference.
    input_cache: dict[Path, tuple[str, tuple[str, ...], frozenset[str]]] = {}
    required_by_object: dict[Path, tuple[str, ...]] = {}
    required_capsules_by_object: dict[Path, tuple[str, ...]] = {}
    project_generated_c_api_by_object: dict[Path, tuple[str, ...]] = {}
    project_generated_prefixes: set[str] = set()
    missing: set[str] = set()
    fail_fast: set[str] = set()
    for object_path, fact in objects_by_path.items():
        capsules: set[str] = set()
        generated_prefixes: set[str] = set()
        inputs = (
            (fact.source_path, fact.source_sha256),
            *((dependency.path, dependency.sha256) for dependency in fact.dependencies),
        )
        for path, expected_digest in inputs:
            resolved = path.resolve()
            cached = input_cache.get(resolved)
            if cached is None:
                try:
                    content = resolved.read_bytes()
                except OSError as exc:
                    return (
                        None,
                        f"failed to read extension object input {resolved}: {exc}",
                    )
                content_digest = hashlib.sha256(content).hexdigest()
                text = content.decode("utf-8", errors="replace")
                cached = (
                    content_digest,
                    _extract_source_extension_required_capsules(text),
                    frozenset(_extract_project_generated_c_api_prefixes(text)),
                )
                input_cache[resolved] = cached
            content_digest, input_capsules, input_prefixes = cached
            if content_digest != expected_digest:
                return None, (
                    "extension object input checksum differs from compiled custody: "
                    f"{resolved} for {object_path}"
                )
            capsules.update(input_capsules)
            generated_prefixes.update(input_prefixes)
        required_capsules_by_object[object_path] = tuple(sorted(capsules))
        project_generated_prefixes.update(generated_prefixes)
        dynamic_python_imports = sorted(
            symbol
            for symbol in fact.undefined_symbols
            if is_cpython_abi_dynamic_import_symbol(symbol)
        )
        if dynamic_python_imports:
            return None, (
                "static extension retains dynamic CPython import-address requirements: "
                + ", ".join(dynamic_python_imports)
            )
        required = sorted(
            symbol
            for symbol in set(fact.undefined_symbols) - project_defined_symbols
            if is_c_api_external_requirement(symbol)
        )
        filtered_required: list[str] = []
        project_generated_required: list[str] = []
        for symbol in required:
            status = scan_surface.status_for(symbol)
            if status == "missing" and _matches_project_generated_c_api_prefix(
                symbol, generated_prefixes
            ):
                project_generated_required.append(symbol)
                continue
            if status == "source_compile_only":
                # Header tokens are compile-time vocabulary, not evidence of a
                # provider for an actual unresolved compiled symbol.
                status = "missing"
            filtered_required.append(symbol)
            if status == "missing":
                missing.add(symbol)
            elif status == "fail_fast":
                fail_fast.add(symbol)
        required_by_object[object_path] = tuple(filtered_required)
        project_generated_c_api_by_object[object_path] = tuple(
            project_generated_required
        )

    return (
        _SourceExtensionCAPIRequirements(
            required_by_object=required_by_object,
            required_capsules_by_object=required_capsules_by_object,
            project_generated_c_api_by_object=project_generated_c_api_by_object,
            project_generated_c_api_prefixes=tuple(sorted(project_generated_prefixes)),
            project_defined_symbols=tuple(sorted(project_defined_symbols)),
            missing_symbols=tuple(sorted(missing)),
            fail_fast_symbols=tuple(sorted(fail_fast)),
        ),
        None,
    )


def _compute_source_extension_object_closure(
    *,
    init_symbol: str,
    object_facts: Sequence[_SourceExtensionObjectFact],
    forced_object_paths: Sequence[Path] = (),
    retained_symbols: Sequence[str] = (),
) -> tuple[_SourceExtensionObjectClosure | None, list[str]]:
    """Resolve all admitted roots through one compiled-object dependency graph."""
    errors: set[str] = set()
    owners: dict[str, list[_SourceExtensionObjectFact]] = {}
    objects_by_path: dict[Path, _SourceExtensionObjectFact] = {}
    for fact in object_facts:
        path = fact.object_path.resolve()
        if path in objects_by_path:
            errors.add(f"source extension object identity is duplicated: {path}")
        objects_by_path[path] = fact
        for symbol in fact.defined_symbols:
            owners.setdefault(symbol, []).append(fact)
    if errors:
        return None, sorted(errors)

    init_owners = owners.get(init_symbol, [])
    if not init_owners:
        return None, [
            f"source extension object closure root {init_symbol!r} is not defined "
            "by any compiled object"
        ]
    if len(init_owners) != 1:
        owner_names = ", ".join(owner.object_path.name for owner in init_owners)
        return None, [
            f"source extension object closure root {init_symbol!r} is ambiguous "
            f"(defined by {owner_names})"
        ]
    if init_symbol not in init_owners[0].defined_function_symbols:
        return None, [
            f"source extension object closure root {init_symbol!r} is not a "
            "function symbol"
        ]

    included: set[Path] = set()
    pending: list[_SourceExtensionObjectFact] = []
    undefined_symbols: set[str] = set()

    def include(fact: _SourceExtensionObjectFact) -> None:
        # Schedule each object only once even with diamonds, cycles, and repeated
        # roots. Preserve source-plan order separately when publishing the result.
        if fact.object_path not in included:
            included.add(fact.object_path)
            pending.append(fact)

    def require(symbol: str) -> None:
        symbol_owners = owners.get(symbol)
        if not symbol_owners:
            # A retained external symbol is a real linker requirement even if
            # no selected source object references it.
            undefined_symbols.add(symbol)
        elif len(symbol_owners) != 1:
            owner_names = ", ".join(owner.object_path.name for owner in symbol_owners)
            errors.add(
                f"source extension symbol {symbol!r} is ambiguously defined by "
                f"{owner_names}"
            )
        else:
            include(symbol_owners[0])

    include(init_owners[0])
    for path in forced_object_paths:
        fact = objects_by_path.get(path.resolve())
        if fact is None:
            errors.add(
                f"source extension forced object {str(path)!r} has no compiled fact"
            )
        else:
            include(fact)
    for symbol in retained_symbols:
        require(symbol)
    while pending:
        fact = pending.pop()
        for symbol in fact.undefined_symbols:
            require(symbol)

    # Forced members may introduce overlapping definitions without any use of
    # the symbol. The current symbol authority has no weak/COMDAT selection
    # facts, so it cannot silently choose a winner.
    for symbol, symbol_owners in owners.items():
        if len(symbol_owners) < 2:
            continue
        selected = [owner for owner in symbol_owners if owner.object_path in included]
        if len(selected) > 1:
            owner_names = ", ".join(owner.object_path.name for owner in selected)
            errors.add(
                f"source extension selected symbol {symbol!r} is ambiguously "
                f"defined by {owner_names}"
            )
    if errors:
        return None, sorted(errors)

    return (
        _SourceExtensionObjectClosure(
            init_symbol=init_symbol,
            init_symbol_owner=init_owners[0],
            objects=tuple(
                fact for fact in object_facts if fact.object_path in included
            ),
            undefined_symbols=tuple(sorted(undefined_symbols)),
        ),
        [],
    )
