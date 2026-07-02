from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.cli.backend_cache import _native_object_global_symbol_sets
from molt.c_api_symbols import is_c_api_external_requirement
from molt.cli.extension_scan_surface import _extract_c_api_tokens
from molt.cli.extension_scan_surface import _extract_file_local_c_api_symbols
from molt.cli.extension_scan_surface import _extract_preprocessor_defined_symbols
from molt.cli.extension_scan_surface import _extract_project_generated_c_api_prefixes
from molt.cli.extension_scan_surface import _extract_project_defined_c_api_symbols
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.extension_scan_surface import _matches_project_generated_c_api_prefix
from molt.cli.extension_scan_surface import _strip_c_like_comments_and_literals
from molt.cli.file_hashing import _sha256_file


_MOLT_NUMPY_ARRAY_API_CAPSULE = "numpy.core._multiarray_umath._ARRAY_API"
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
_SOURCE_EXTENSION_HEADER_SUFFIXES = {
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".inc",
}
_SOURCE_EXTENSION_TARGET_OUTPUT_SUFFIXES = (
    ".molt.wasm",
    ".pyd",
    ".so",
    ".dll",
    ".dylib",
)
_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX = (
    "extension_manifest.json source missing:"
)
_MESON_EXTENSION_TARGET_TYPES = {"shared module", "shared library", "library"}


@dataclass(frozen=True)
class _SourceExtensionObjectFact:
    source_path: Path
    object_path: Path
    source_sha256: str
    object_sha256: str
    defined_symbols: tuple[str, ...]
    undefined_symbols: tuple[str, ...]

    def manifest_payload(
        self,
        *,
        required_c_api_symbols: Sequence[str] = (),
        required_capsules: Sequence[str] = (),
        project_generated_c_api_symbols: Sequence[str] = (),
    ) -> dict[str, Any]:
        return {
            "source": str(self.source_path),
            "object": self.object_path.name,
            "source_sha256": self.source_sha256,
            "object_sha256": self.object_sha256,
            "defined_symbols": list(self.defined_symbols),
            "undefined_symbols": list(self.undefined_symbols),
            "required_c_api_symbols": list(required_c_api_symbols),
            "required_capsules": list(required_capsules),
            "project_generated_c_api_symbols": list(project_generated_c_api_symbols),
        }


@dataclass(frozen=True)
class _SourceExtensionObjectClosure:
    init_symbol: str
    init_symbol_owner: _SourceExtensionObjectFact
    objects: tuple[_SourceExtensionObjectFact, ...]
    runtime_symbols: tuple[str, ...]
    closure_sha256: str

    def manifest_payload(
        self,
        *,
        required_c_api_by_source: Mapping[Path, Sequence[str]] | None = None,
        required_capsules_by_source: Mapping[Path, Sequence[str]] | None = None,
        project_generated_c_api_by_source: Mapping[Path, Sequence[str]] | None = None,
        project_generated_c_api_prefixes: Sequence[str] = (),
    ) -> dict[str, Any]:
        c_api_by_source = required_c_api_by_source or {}
        capsules_by_source = required_capsules_by_source or {}
        generated_by_source = project_generated_c_api_by_source or {}
        required_capsules = sorted(
            {
                capsule
                for fact in self.objects
                for capsule in capsules_by_source.get(fact.source_path.resolve(), ())
            }
        )
        return {
            "schema_version": 1,
            "root_symbol": self.init_symbol,
            "init_symbol_owner": self.init_symbol_owner.object_path.name,
            "closure_sha256": self.closure_sha256,
            "runtime_symbols": list(self.runtime_symbols),
            "required_capsules": required_capsules,
            "project_generated_c_api_prefixes": list(
                sorted(project_generated_c_api_prefixes)
            ),
            "objects": [
                fact.manifest_payload(
                    required_c_api_symbols=tuple(
                        c_api_by_source.get(fact.source_path.resolve(), ())
                    ),
                    required_capsules=tuple(
                        capsules_by_source.get(fact.source_path.resolve(), ())
                    ),
                    project_generated_c_api_symbols=tuple(
                        generated_by_source.get(fact.source_path.resolve(), ())
                    ),
                )
                for fact in self.objects
            ],
        }


@dataclass(frozen=True)
class _SourceExtensionCAPIRequirements:
    required_by_source: dict[Path, tuple[str, ...]]
    required_capsules_by_source: dict[Path, tuple[str, ...]]
    project_generated_c_api_by_source: dict[Path, tuple[str, ...]]
    project_generated_c_api_prefixes: tuple[str, ...]
    project_defined_symbols: tuple[str, ...]
    missing_symbols: tuple[str, ...]
    fail_fast_symbols: tuple[str, ...]

    def manifest_payload(self) -> dict[str, Any]:
        project_generated_symbols = sorted(
            {
                symbol
                for symbols in self.project_generated_c_api_by_source.values()
                for symbol in symbols
            }
        )
        return {
            "required_symbol_count": sum(
                len(symbols) for symbols in self.required_by_source.values()
            ),
            "required_capsule_count": sum(
                len(capsules) for capsules in self.required_capsules_by_source.values()
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
    generated: bool
    language: str | None
    compiler: tuple[str, ...]
    include_dirs: tuple[Path, ...]
    compile_args: tuple[str, ...]

    def manifest_payload(self) -> dict[str, Any]:
        return {
            "source": str(self.source_path),
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
    target_type: str
    source_root: Path
    build_root: Path
    sources: tuple[Path, ...]
    generated_sources: tuple[Path, ...]
    non_compiled_inputs: tuple[Path, ...]
    compile_units: tuple[_SourceExtensionCompileUnit, ...]
    include_dirs: tuple[Path, ...]
    compile_args: tuple[str, ...]
    link_args: tuple[str, ...]
    digest: str

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
            "target_type": self.target_type,
            "source_root": str(self.source_root),
            "build_root": str(self.build_root),
            "digest": self.digest,
            "sources": [str(path) for path in self.sources],
            "generated_sources": [str(path) for path in self.generated_sources],
            "non_compiled_inputs": [str(path) for path in self.non_compiled_inputs],
            "compile_units": [unit.manifest_payload() for unit in self.compile_units],
            "include_dirs": [str(path) for path in self.include_dirs],
            "compile_args": list(self.compile_args),
            "link_args": list(self.link_args),
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
    raw_args = target.get("linker_parameters") or target.get("link_args") or []
    if not isinstance(raw_args, list):
        return ()
    return tuple(str(arg) for arg in raw_args)


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


def _meson_target_object_roots(filename: Any, *, build_root: Path) -> tuple[Path, ...]:
    raw_filenames = filename if isinstance(filename, list) else (filename,)
    roots: list[Path] = []
    for raw_filename in raw_filenames:
        if not isinstance(raw_filename, str) or not raw_filename.strip():
            continue
        target_path = Path(raw_filename).expanduser()
        if not target_path.is_absolute():
            target_path = build_root / target_path
        target_path = target_path.resolve()
        roots.append((target_path.parent / f"{target_path.name}.p").resolve())
    return _dedupe_paths(roots)


def _source_extension_build_plan_digest(plan: _SourceExtensionBuildPlan) -> str:
    payload = {
        "kind": plan.kind,
        "plan_sha256": plan.plan_sha256,
        "compile_commands_sha256": plan.compile_commands_sha256,
        "target_id": plan.target_id,
        "target_name": plan.target_name,
        "target_type": plan.target_type,
        "source_root": str(plan.source_root),
        "build_root": str(plan.build_root),
        "sources": [str(path) for path in plan.sources],
        "generated_sources": [str(path) for path in plan.generated_sources],
        "non_compiled_inputs": [str(path) for path in plan.non_compiled_inputs],
        "compile_units": [
            {
                "source": str(unit.source_path),
                "generated": unit.generated,
                "language": unit.language,
                "compiler": list(unit.compiler),
                "include_dirs": [str(path) for path in unit.include_dirs],
                "compile_args": list(unit.compile_args),
            }
            for unit in plan.compile_units
        ],
        "include_dirs": [str(path) for path in plan.include_dirs],
        "compile_args": list(plan.compile_args),
        "link_args": list(plan.link_args),
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
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg in {"-c", "/c"}:
            idx += 1
            continue
        if arg in {"-o", "/Fo", "-MF", "-MT", "-MQ"}:
            idx += 2
            continue
        if (
            arg.startswith("-o")
            or arg.startswith("/Fo")
            or arg.startswith("-MF")
            or arg.startswith("-MT")
            or arg.startswith("-MQ")
        ) and len(arg) > 2:
            idx += 1
            continue
        if arg in {"-MD", "-MMD", "-MP"}:
            idx += 1
            continue
        try:
            if _resolve_compile_command_path(arg, directory=directory) == source_path:
                idx += 1
                continue
        except OSError:
            pass
        semantic_args.append(arg)
        idx += 1
    return semantic_args


def _compile_command_output_path(
    arguments: Sequence[str],
    *,
    directory: Path,
) -> Path | None:
    if not arguments:
        return None
    _compiler, args = _compile_command_compiler_and_args(arguments)
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg in {"-o", "/Fo"} and idx + 1 < len(args):
            return _resolve_compile_command_path(args[idx + 1], directory=directory)
        if arg.startswith("-o") and len(arg) > 2:
            return _resolve_compile_command_path(arg[2:], directory=directory)
        if arg.startswith("/Fo") and len(arg) > 3:
            return _resolve_compile_command_path(arg[3:], directory=directory)
        idx += 1
    return None


def _path_is_within(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
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
    items = [str(item) for item in arguments]
    idx = 0
    while idx < len(items):
        item = items[idx]
        if item == "-I" and idx + 1 < len(items):
            include_dirs.append(
                _resolve_compile_command_path(items[idx + 1], directory=directory)
            )
            idx += 2
            continue
        if item.startswith("-I") and len(item) > 2:
            include_dirs.append(
                _resolve_compile_command_path(item[2:], directory=directory)
            )
            idx += 1
            continue
        if item == "/I" and idx + 1 < len(items):
            include_dirs.append(
                _resolve_compile_command_path(items[idx + 1], directory=directory)
            )
            idx += 2
            continue
        if item.startswith("/I") and len(item) > 2:
            include_dirs.append(
                _resolve_compile_command_path(item[2:], directory=directory)
            )
            idx += 1
            continue
        if item in {"-isystem", "-iquote"} and idx + 1 < len(items):
            compile_args.append(item)
            compile_args.append(
                str(_resolve_compile_command_path(items[idx + 1], directory=directory))
            )
            idx += 2
            continue
        compile_args.append(item)
        idx += 1
    return tuple(compile_args), _dedupe_paths(include_dirs)


def _load_compile_command_units(
    compile_commands_path: Path,
    *,
    required_sources: set[Path] | None = None,
    target_output_roots: Sequence[Path] = (),
) -> tuple[
    dict[Path, tuple[tuple[str, ...], tuple[str, ...], tuple[Path, ...]]] | None,
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
    preferred_output_roots = _dedupe_paths(
        [root.resolve() for root in target_output_roots]
    )
    candidates_by_source: dict[
        Path,
        list[tuple[tuple[tuple[str, ...], tuple[str, ...], tuple[Path, ...]], bool]],
    ] = {}
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
        if (
            required_source_paths is not None
            and source_path not in required_source_paths
        ):
            continue
        arguments = _compile_command_arguments(entry)
        if arguments is None:
            errors.append(f"compile command for {source_path} lacks arguments/command")
            continue
        compiler, _compiler_args = _compile_command_compiler_and_args(arguments)
        output_path = _compile_command_output_path(arguments, directory=directory)
        target_owned = (
            output_path is not None
            and bool(preferred_output_roots)
            and any(
                _path_is_within(output_path, root) for root in preferred_output_roots
            )
        )
        semantic_args = _compile_command_semantic_args(
            arguments,
            source_path=source_path,
            directory=directory,
        )
        compile_args, include_dirs = _compile_command_args_and_include_dirs(
            semantic_args,
            directory=directory,
        )
        unit = (compiler, compile_args, include_dirs)
        candidates_by_source.setdefault(source_path, []).append((unit, target_owned))

    commands_by_source: dict[
        Path, tuple[tuple[str, ...], tuple[str, ...], tuple[Path, ...]]
    ] = {}
    for source_path, candidates in candidates_by_source.items():
        target_owned_units = [unit for unit, target_owned in candidates if target_owned]
        selected_units = target_owned_units or [
            unit for unit, _target_owned in candidates
        ]
        unique_units: list[
            tuple[tuple[str, ...], tuple[str, ...], tuple[Path, ...]]
        ] = []
        for unit in selected_units:
            if unit not in unique_units:
                unique_units.append(unit)
        if len(unique_units) > 1:
            errors.append(
                f"compile_commands.json has conflicting entries for {source_path}"
            )
            continue
        commands_by_source[source_path] = unique_units[0]
    if errors:
        return None, errors
    return commands_by_source, []


def _load_meson_intro_targets_source_extension_plan(
    *,
    plan_path: Path,
    project_root: Path,
    module_name: str,
    selector: Any = None,
    source_root: Any = None,
    build_root: Any = None,
    compile_commands: Any = None,
) -> tuple[_SourceExtensionBuildPlan | None, list[str]]:
    errors: list[str] = []
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
    matches: list[Mapping[str, Any]] = []
    for entry in payload:
        if not isinstance(entry, Mapping):
            continue
        names = {
            str(entry.get("id", "")),
            str(entry.get("name", "")),
        }
        names.update(_meson_target_filename_names(entry.get("filename")))
        if selected in names:
            matches.append(entry)
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
    target_output_roots = _meson_target_object_roots(
        target.get("filename"),
        build_root=resolved_build_root,
    )

    target_compile_command_sources: list[Path] = []
    for source_group in target_sources:
        if not isinstance(source_group, Mapping):
            continue
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
        compile_command_units = {}

    sources: list[Path] = []
    generated_sources: list[Path] = []
    non_compiled_inputs: list[Path] = []
    compile_units: list[_SourceExtensionCompileUnit] = []
    compile_units_by_source: dict[Path, _SourceExtensionCompileUnit] = {}
    include_dirs: list[Path] = []
    compile_args: list[str] = []

    def append_compile_unit(
        *,
        source_path: Path,
        generated: bool,
        language: str | None,
        unit_compiler: tuple[str, ...],
        unit_includes: tuple[Path, ...],
        unit_args: tuple[str, ...],
    ) -> None:
        resolved_source_path = source_path.resolve()
        unit = _SourceExtensionCompileUnit(
            source_path=resolved_source_path,
            generated=generated,
            language=language,
            compiler=unit_compiler,
            include_dirs=unit_includes,
            compile_args=unit_args,
        )
        existing = compile_units_by_source.get(resolved_source_path)
        if existing is not None:
            if existing != unit:
                errors.append(
                    "Meson target lists compiled source with conflicting metadata: "
                    f"{resolved_source_path}"
                )
            return
        compile_units_by_source[resolved_source_path] = unit
        compile_units.append(unit)
        compile_args.extend(unit_args)
        include_dirs.extend(unit_includes)

    for source_group in target_sources:
        if not isinstance(source_group, Mapping):
            continue
        language_value = source_group.get("language")
        language = (
            language_value.strip()
            if isinstance(language_value, str) and language_value.strip()
            else None
        )
        for raw_source in source_group.get("sources") or ():
            source_path = _resolve_meson_plan_artifact_path(
                raw_source,
                source_root=resolved_source_root,
                build_root=resolved_build_root,
                prefer_build_root=False,
            )
            if _is_compilable_source_path(source_path):
                command_unit = compile_command_units.get(source_path.resolve())
                if command_unit is None:
                    errors.append(
                        "compile_commands.json has no entry for Meson target "
                        f"source: {source_path.resolve()}"
                    )
                    unit_compiler: tuple[str, ...] = ()
                    unit_args: tuple[str, ...] = ()
                    unit_includes: tuple[Path, ...] = ()
                else:
                    unit_compiler, unit_args, unit_includes = command_unit
                sources.append(source_path)
                append_compile_unit(
                    source_path=source_path,
                    generated=False,
                    language=language,
                    unit_compiler=unit_compiler,
                    unit_includes=unit_includes,
                    unit_args=unit_args,
                )
            else:
                non_compiled_inputs.append(source_path)
        for raw_source in source_group.get("generated_sources") or ():
            source_path = _resolve_meson_plan_artifact_path(
                raw_source,
                source_root=resolved_source_root,
                build_root=resolved_build_root,
                prefer_build_root=True,
            )
            if _is_compilable_source_path(source_path):
                command_unit = compile_command_units.get(source_path.resolve())
                if command_unit is None:
                    errors.append(
                        "compile_commands.json has no entry for Meson target "
                        f"generated source: {source_path.resolve()}"
                    )
                    unit_compiler = ()
                    unit_args = ()
                    unit_includes = ()
                else:
                    unit_compiler, unit_args, unit_includes = command_unit
                generated_sources.append(source_path)
                append_compile_unit(
                    source_path=source_path,
                    generated=True,
                    language=language,
                    unit_compiler=unit_compiler,
                    unit_includes=unit_includes,
                    unit_args=unit_args,
                )
            else:
                non_compiled_inputs.append(source_path)

    deduped_sources = _dedupe_paths(sources)
    deduped_generated_sources = _dedupe_paths(generated_sources)
    all_sources = (*deduped_sources, *deduped_generated_sources)
    if not all_sources:
        errors.append(
            f"Meson target {target_name or target_id!r} does not expose any "
            "compiled C/C++/Objective-C source units"
        )
    for source_path in all_sources:
        if not source_path.exists() or not source_path.is_file():
            errors.append(f"Meson target source does not exist: {source_path}")
    deduped_non_compiled_inputs = _dedupe_paths(non_compiled_inputs)
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
        target_type=target_type,
        source_root=resolved_source_root,
        build_root=resolved_build_root,
        sources=deduped_sources,
        generated_sources=deduped_generated_sources,
        non_compiled_inputs=deduped_non_compiled_inputs,
        compile_units=tuple(compile_units),
        include_dirs=_dedupe_paths(include_dirs),
        compile_args=tuple(compile_args),
        link_args=_meson_link_args(target),
        digest="",
    )
    return (
        _SourceExtensionBuildPlan(
            kind=plan.kind,
            plan_path=plan.plan_path,
            plan_sha256=plan.plan_sha256,
            compile_commands_path=plan.compile_commands_path,
            compile_commands_sha256=plan.compile_commands_sha256,
            target_id=plan.target_id,
            target_name=plan.target_name,
            target_type=plan.target_type,
            source_root=plan.source_root,
            build_root=plan.build_root,
            sources=plan.sources,
            generated_sources=plan.generated_sources,
            non_compiled_inputs=plan.non_compiled_inputs,
            compile_units=plan.compile_units,
            include_dirs=plan.include_dirs,
            compile_args=plan.compile_args,
            link_args=plan.link_args,
            digest=_source_extension_build_plan_digest(plan),
        ),
        [],
    )


def _load_source_extension_build_plan(
    *,
    project_root: Path,
    module_name: str,
    plan_config: Mapping[str, Any],
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
        )
    return None, [f"unsupported source extension build plan kind: {kind!r}"]


def _source_extension_compile_unit_mentions_target(
    unit: _SourceExtensionCompileUnit,
    *,
    target_triple: str,
) -> bool:
    target = target_triple.lower()
    if target.startswith("wasm32"):
        target_markers = ("wasm32", "wasip1", "wasi")
    else:
        target_markers = tuple(part for part in target.split("-") if part)
    tokens = [str(token).lower() for token in (*unit.compiler, *unit.compile_args)]
    if any(any(marker in token for marker in target_markers) for token in tokens):
        return True
    for idx, token in enumerate(tokens[:-1]):
        if token in {"-target", "--target"} and any(
            marker in tokens[idx + 1] for marker in target_markers
        ):
            return True
        for prefix in ("-target=", "--target="):
            if token.startswith(prefix) and any(
                marker in token[len(prefix) :] for marker in target_markers
            ):
                return True
    return False


def _validate_source_extension_build_plan_target(
    plan: _SourceExtensionBuildPlan,
    *,
    target_triple: str | None,
) -> list[str]:
    if target_triple is None or not target_triple.lower().startswith("wasm32"):
        return []
    nonmatching = [
        unit
        for unit in plan.compile_units
        if not _source_extension_compile_unit_mentions_target(
            unit,
            target_triple=target_triple,
        )
    ]
    if not nonmatching:
        return []
    preview = ", ".join(str(unit.source_path) for unit in nonmatching[:4])
    suffix = "" if len(nonmatching) <= 4 else ", ..."
    return [
        "WASM source-extension builds require a target-specific upstream "
        "compile_commands.json; selected compile command rows do not mention "
        f"{target_triple}: {preview}{suffix}. Configure the upstream package "
        "for wasm32 and pass that build root/compile database instead of "
        "reusing native build metadata."
    ]


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
        return ["-mexception-handling", "-mllvm", "-wasm-enable-sjlj"]
    return []


def _source_extension_gc_link_args(
    *,
    cc_cmd: Sequence[str],
    target_triple: str | None,
) -> list[str]:
    target = (target_triple or "").lower()
    tool = Path(cc_cmd[0]).name.lower() if cc_cmd else ""
    if "windows" in target or (not target and os.name == "nt"):
        if tool in {"cl", "cl.exe", "clang-cl", "clang-cl.exe"}:
            return ["/link", "/OPT:REF"]
        return ["-Wl,/OPT:REF"]
    if "darwin" in target or (not target and sys.platform == "darwin"):
        return ["-Wl,-dead_strip"]
    return ["-Wl,--gc-sections"]


def _source_extension_object_fact(
    *,
    source_path: Path,
    object_path: Path,
) -> tuple[_SourceExtensionObjectFact | None, str | None]:
    symbol_sets = _native_object_global_symbol_sets(object_path)
    if symbol_sets is None:
        return (
            None,
            "unable to read global symbol table for compiled extension object "
            f"{object_path}; install llvm-nm/nm or set MOLT_NM",
        )
    defined, undefined = symbol_sets
    return (
        _SourceExtensionObjectFact(
            source_path=source_path.resolve(),
            object_path=object_path,
            source_sha256=_sha256_file(source_path),
            object_sha256=_sha256_file(object_path),
            defined_symbols=tuple(sorted(defined)),
            undefined_symbols=tuple(sorted(undefined)),
        ),
        None,
    )


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


def _manifest_source_plan_relocation_roots(
    manifest: Mapping[str, Any],
) -> tuple[Path, ...]:
    source_plan = manifest.get("source_plan")
    if not isinstance(source_plan, Mapping):
        return ()
    roots: list[Path] = []
    for field_name in ("source_root", "build_root"):
        source_root = source_plan.get(field_name)
        if isinstance(source_root, str) and source_root.strip():
            roots.append(Path(source_root).expanduser())
    return tuple(roots)


def _source_extension_relocation_roots(
    source_root: Path,
    *,
    manifest_path: Path,
) -> tuple[Path, ...]:
    candidates: list[Path] = []
    if source_root.exists():
        candidates.append(source_root)
    search_bases = [manifest_path.parent, *manifest_path.parents, Path.cwd()]
    for base in search_bases:
        candidates.append(base / source_root.name)
    parts = source_root.parts
    if "bench" in parts:
        suffix = Path(*parts[parts.index("bench") :])
        for base in search_bases:
            candidates.append(base / suffix)
    for index, part in enumerate(parts):
        if part == "tmp":
            suffix = Path(*parts[index:])
            for base in search_bases:
                candidates.append(base / suffix)
    seen: set[Path] = set()
    unique: list[Path] = []
    for candidate in candidates:
        resolved = candidate.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        unique.append(resolved)
    return tuple(unique)


def _source_extension_hash_matches(path: Path, expected_sha256: str | None) -> bool:
    if not expected_sha256:
        return True
    return _sha256_file(path) == expected_sha256


def _source_extension_missing_source_error(field_name: str, path: Path) -> str:
    return f"{_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX} {field_name}: {path}"


def source_extension_manifest_errors_are_missing_sources(
    errors: Sequence[str],
) -> bool:
    return bool(errors) and all(
        error.startswith(_SOURCE_EXTENSION_MISSING_SOURCE_ERROR_PREFIX)
        for error in errors
    )


def source_extension_manifest_source_path(
    raw_path: str,
    *,
    manifest: Mapping[str, Any],
    manifest_path: Path,
    expected_sha256: str | None = None,
) -> tuple[Path | None, list[str]]:
    source_path = source_extension_manifest_path(raw_path, manifest_path=manifest_path)
    if source_path.is_file():
        if not _source_extension_hash_matches(source_path, expected_sha256):
            return None, [
                f"extension_manifest.json source checksum mismatch: {source_path}"
            ]
        return source_path, []

    raw_source_path = Path(raw_path).expanduser()
    source_roots = _manifest_source_plan_relocation_roots(manifest)
    if not source_roots:
        return None, []
    relative_roots: list[tuple[Path, Path]] = []
    for source_root in source_roots:
        try:
            relative_roots.append(
                (source_root, raw_source_path.relative_to(source_root))
            )
        except ValueError:
            continue
    if not relative_roots:
        return None, []
    mismatched_candidates: list[Path] = []
    for source_root, relative_source in relative_roots:
        for root in _source_extension_relocation_roots(
            source_root,
            manifest_path=manifest_path,
        ):
            candidate = (root / relative_source).resolve()
            if not candidate.is_file():
                continue
            if not _source_extension_hash_matches(candidate, expected_sha256):
                mismatched_candidates.append(candidate)
                continue
            return candidate, []
    if mismatched_candidates:
        return None, [
            "extension_manifest.json relocated source checksum mismatch: "
            + ", ".join(str(path) for path in mismatched_candidates[:3])
        ]
    return None, []


def _source_extension_object_source_sha256(
    objects: Any,
) -> dict[str, str]:
    if not isinstance(objects, list):
        return {}
    by_source: dict[str, str] = {}
    for item in objects:
        if not isinstance(item, Mapping):
            continue
        source = item.get("source")
        source_sha256 = item.get("source_sha256")
        if (
            isinstance(source, str)
            and source.strip()
            and isinstance(source_sha256, str)
            and source_sha256.strip()
        ):
            by_source[source] = source_sha256.strip()
    return by_source


def _resolve_source_extension_manifest_source(
    raw_source: str,
    *,
    manifest: Mapping[str, Any],
    manifest_path: Path,
    expected_sha256: str | None,
    field_name: str,
) -> tuple[Path | None, list[str]]:
    source_path, errors = source_extension_manifest_source_path(
        raw_source,
        manifest=manifest,
        manifest_path=manifest_path,
        expected_sha256=expected_sha256,
    )
    if errors:
        return None, errors
    if source_path is None:
        return None, [
            _source_extension_missing_source_error(
                field_name,
                source_extension_manifest_path(
                    raw_source,
                    manifest_path=manifest_path,
                ),
            )
        ]
    return source_path, []


def _dedupe_source_extension_manifest_paths(paths: Sequence[Path]) -> tuple[Path, ...]:
    seen: set[Path] = set()
    unique: list[Path] = []
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        unique.append(path)
    return tuple(unique)


def _source_extension_manifest_source_paths(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
) -> tuple[tuple[Path, ...] | None, list[str]]:
    errors: list[str] = []
    paths: list[Path] = []
    object_closure = manifest.get("object_closure")
    objects = (
        object_closure.get("objects") if isinstance(object_closure, Mapping) else None
    )
    source_sha256_by_raw = _source_extension_object_source_sha256(objects)
    if isinstance(object_closure, Mapping):
        if objects is not None:
            if not isinstance(objects, list):
                errors.append(
                    "extension_manifest.json object_closure.objects must be a list"
                )
            for index, item in enumerate(objects if isinstance(objects, list) else ()):
                if not isinstance(item, Mapping):
                    continue
                source = item.get("source")
                if isinstance(source, str) and source.strip():
                    source_sha256 = item.get("source_sha256")
                    source_path, source_errors = (
                        _resolve_source_extension_manifest_source(
                            source,
                            manifest=manifest,
                            manifest_path=manifest_path,
                            expected_sha256=(
                                source_sha256.strip()
                                if isinstance(source_sha256, str)
                                and source_sha256.strip()
                                else None
                            ),
                            field_name=f"object_closure.objects[{index}].source",
                        )
                    )
                    errors.extend(source_errors)
                    if source_path is not None:
                        paths.append(source_path)
    if errors:
        return None, errors
    if paths:
        return _dedupe_source_extension_manifest_paths(paths), []

    raw_sources = manifest.get("sources")
    if raw_sources is not None:
        if not isinstance(raw_sources, list):
            errors.append("extension_manifest.json sources must be a list of paths")
        else:
            for index, raw_source in enumerate(raw_sources):
                if not isinstance(raw_source, str) or not raw_source.strip():
                    errors.append(
                        "extension_manifest.json sources must be a list of paths"
                    )
                    continue
                source_path, source_errors = _resolve_source_extension_manifest_source(
                    raw_source,
                    manifest=manifest,
                    manifest_path=manifest_path,
                    expected_sha256=source_sha256_by_raw.get(raw_source),
                    field_name=f"sources[{index}]",
                )
                errors.extend(source_errors)
                if source_path is not None:
                    paths.append(source_path)
    if errors:
        return None, errors
    return _dedupe_source_extension_manifest_paths(paths), []


def source_extension_manifest_required_capsule_imports_by_source(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
) -> tuple[dict[Path, dict[str, tuple[str, ...]]] | None, list[str]]:
    sources, source_errors = _source_extension_manifest_source_paths(
        manifest,
        manifest_path=manifest_path,
    )
    if source_errors:
        return None, source_errors
    assert sources is not None
    by_source: dict[Path, dict[str, tuple[str, ...]]] = {}
    for source_path in sources:
        try:
            source_text = source_path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            return None, [
                "cannot verify source-derived capsule requirements because "
                f"manifest source {source_path} is unreadable: {exc}"
            ]
        required = source_extension_required_capsule_imports(source_text)
        if required:
            by_source[source_path] = required
    return by_source, []


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


def _source_extension_definition_header_paths(
    header_roots: Sequence[Path],
) -> tuple[Path, ...]:
    seen: set[Path] = set()
    headers: list[Path] = []
    for header_root in header_roots:
        root = header_root.resolve()
        if not root.exists():
            continue
        if root.is_file():
            candidates = (root,)
        else:
            candidates = tuple(
                path
                for path in root.rglob("*")
                if path.is_file()
                and path.suffix.lower() in _SOURCE_EXTENSION_HEADER_SUFFIXES
            )
        for candidate in candidates:
            resolved = candidate.resolve()
            if resolved in seen:
                continue
            seen.add(resolved)
            headers.append(resolved)
    return tuple(headers)


def _source_extension_definition_header_texts(
    header_roots: Sequence[Path],
) -> tuple[dict[Path, str] | None, str | None]:
    texts: dict[Path, str] = {}
    for header_path in _source_extension_definition_header_paths(header_roots):
        try:
            texts[header_path] = header_path.read_text(
                encoding="utf-8",
                errors="replace",
            )
        except (OSError, UnicodeError) as exc:
            return None, f"failed to read extension header {header_path}: {exc}"
    return texts, None


def _source_extension_project_defined_c_api_symbols(
    *,
    source_text_by_path: Mapping[Path, str],
    definition_header_text_by_path: Mapping[Path, str],
) -> tuple[set[str] | None, str | None]:
    project_defined_symbols: set[str] = set()
    for source_text in source_text_by_path.values():
        project_defined_symbols.update(
            _extract_project_defined_c_api_symbols(source_text)
        )
    for header_text in definition_header_text_by_path.values():
        project_defined_symbols.update(
            _extract_project_defined_c_api_symbols(
                header_text,
                include_static_inline=True,
                include_declarations=True,
            )
        )
    return project_defined_symbols, None


def _source_extension_project_generated_c_api_prefixes(
    *,
    source_text_by_path: Mapping[Path, str],
    definition_header_text_by_path: Mapping[Path, str],
) -> tuple[str, ...]:
    prefixes: set[str] = set()
    for source_text in source_text_by_path.values():
        prefixes.update(_extract_project_generated_c_api_prefixes(source_text))
    for header_text in definition_header_text_by_path.values():
        prefixes.update(_extract_project_generated_c_api_prefixes(header_text))
    return tuple(sorted(prefixes))


def _source_extension_compile_arg_preprocessor_symbols(
    compile_args: Sequence[str],
) -> tuple[set[str], set[str]]:
    defined: set[str] = set()
    undefined: set[str] = set()
    items = [str(item) for item in compile_args]
    idx = 0
    while idx < len(items):
        item = items[idx]
        raw_define: str | None = None
        raw_undef: str | None = None
        if item in {"-D", "/D"} and idx + 1 < len(items):
            raw_define = items[idx + 1]
            idx += 2
        elif item in {"-U", "/U"} and idx + 1 < len(items):
            raw_undef = items[idx + 1]
            idx += 2
        elif item.startswith("-D") and len(item) > 2:
            raw_define = item[2:]
            idx += 1
        elif item.startswith("/D") and len(item) > 2:
            raw_define = item[2:]
            idx += 1
        elif item.startswith("-U") and len(item) > 2:
            raw_undef = item[2:]
            idx += 1
        elif item.startswith("/U") and len(item) > 2:
            raw_undef = item[2:]
            idx += 1
        elif item.startswith("-Wp,"):
            for part in item.split(",")[1:]:
                if part.startswith("-D") and len(part) > 2:
                    symbol = part[2:].split("=", 1)[0]
                    if _C_IDENTIFIER_RE.fullmatch(symbol):
                        defined.add(symbol)
                elif part.startswith("-U") and len(part) > 2:
                    symbol = part[2:].split("=", 1)[0]
                    if _C_IDENTIFIER_RE.fullmatch(symbol):
                        undefined.add(symbol)
            idx += 1
        else:
            idx += 1

        if raw_define is not None:
            symbol = raw_define.split("=", 1)[0]
            if _C_IDENTIFIER_RE.fullmatch(symbol):
                defined.add(symbol)
                undefined.discard(symbol)
        if raw_undef is not None:
            symbol = raw_undef.split("=", 1)[0]
            if _C_IDENTIFIER_RE.fullmatch(symbol):
                undefined.add(symbol)
                defined.discard(symbol)
    return defined, undefined


def _source_extension_global_preprocessor_symbols(
    *,
    definition_header_text_by_path: Mapping[Path, str],
    explicit_symbols: Sequence[str],
) -> frozenset[str]:
    symbols = {
        str(symbol)
        for symbol in explicit_symbols
        if _C_IDENTIFIER_RE.fullmatch(str(symbol))
    }
    for header_text in definition_header_text_by_path.values():
        symbols.update(_extract_preprocessor_defined_symbols(header_text))
    return frozenset(symbols)


def _source_extension_required_c_api_by_source(
    *,
    molt_root: Path,
    source_paths: Sequence[Path],
    python_header: Path | None = None,
    definition_header_roots: Sequence[Path] = (),
    compile_args_by_source: Mapping[Path, Sequence[str]] | None = None,
    preprocessor_defined_symbols: Sequence[str] = (),
) -> tuple[_SourceExtensionCAPIRequirements | None, str | None]:
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

    source_text_by_path: dict[Path, str] = {}
    for source_path in source_paths:
        resolved = source_path.resolve()
        try:
            source_text = resolved.read_text(encoding="utf-8", errors="replace")
        except (OSError, UnicodeError) as exc:
            return (
                None,
                f"failed to read extension source {resolved}: {exc}",
            )
        source_text_by_path[resolved] = source_text
    definition_header_text_by_path, header_text_error = (
        _source_extension_definition_header_texts(definition_header_roots)
    )
    if header_text_error is not None:
        return None, header_text_error
    assert definition_header_text_by_path is not None
    global_preprocessor_symbols = _source_extension_global_preprocessor_symbols(
        definition_header_text_by_path=definition_header_text_by_path,
        explicit_symbols=preprocessor_defined_symbols,
    )
    compile_args_by_resolved_source = {
        path.resolve(): tuple(args)
        for path, args in (compile_args_by_source or {}).items()
    }
    active_preprocessor_symbols_by_source: dict[Path, frozenset[str]] = {}
    file_local_symbols_by_path: dict[Path, set[str]] = {}
    for source_path, source_text in source_text_by_path.items():
        active_symbols = set(global_preprocessor_symbols)
        defined_by_args, undefined_by_args = (
            _source_extension_compile_arg_preprocessor_symbols(
                compile_args_by_resolved_source.get(source_path, ())
            )
        )
        active_symbols.update(defined_by_args)
        active_symbols.difference_update(undefined_by_args)
        active_preprocessor_symbols = frozenset(active_symbols)
        active_preprocessor_symbols_by_source[source_path] = active_preprocessor_symbols
        file_local_symbols_by_path[source_path] = _extract_file_local_c_api_symbols(
            source_text,
            active_preprocessor_symbols=active_preprocessor_symbols,
        )
    project_defined_symbols, defined_error = (
        _source_extension_project_defined_c_api_symbols(
            source_text_by_path=source_text_by_path,
            definition_header_text_by_path=definition_header_text_by_path,
        )
    )
    if defined_error is not None:
        return None, defined_error
    assert project_defined_symbols is not None
    project_generated_c_api_prefixes = (
        _source_extension_project_generated_c_api_prefixes(
            source_text_by_path=source_text_by_path,
            definition_header_text_by_path=definition_header_text_by_path,
        )
    )
    generated_prefixes = frozenset(project_generated_c_api_prefixes)

    required_by_source: dict[Path, tuple[str, ...]] = {}
    required_capsules_by_source: dict[Path, tuple[str, ...]] = {}
    project_generated_c_api_by_source: dict[Path, tuple[str, ...]] = {}
    missing: set[str] = set()
    fail_fast: set[str] = set()
    for source_path, source_text in source_text_by_path.items():
        required_capsules_by_source[source_path] = (
            _extract_source_extension_required_capsules(source_text)
        )
        required = tuple(
            sorted(
                {
                    symbol
                    for symbol in (
                        _extract_c_api_tokens(
                            source_text,
                            active_preprocessor_symbols=(
                                active_preprocessor_symbols_by_source[source_path]
                            ),
                        )
                        - file_local_symbols_by_path[source_path]
                    )
                    if is_c_api_external_requirement(symbol)
                }
            )
        )
        filtered_required: list[str] = []
        project_generated_required: list[str] = []
        for symbol in required:
            if symbol in project_defined_symbols:
                continue
            status = scan_surface.status_for(symbol)
            if status == "missing" and _matches_project_generated_c_api_prefix(
                symbol,
                generated_prefixes,
            ):
                project_generated_required.append(symbol)
                continue
            filtered_required.append(symbol)
            if status == "missing":
                missing.add(symbol)
            elif status == "fail_fast":
                fail_fast.add(symbol)
        required_by_source[source_path] = tuple(filtered_required)
        project_generated_c_api_by_source[source_path] = tuple(
            sorted(project_generated_required)
        )

    return (
        _SourceExtensionCAPIRequirements(
            required_by_source=required_by_source,
            required_capsules_by_source=required_capsules_by_source,
            project_generated_c_api_by_source=project_generated_c_api_by_source,
            project_generated_c_api_prefixes=project_generated_c_api_prefixes,
            project_defined_symbols=tuple(sorted(project_defined_symbols)),
            missing_symbols=tuple(sorted(missing)),
            fail_fast_symbols=tuple(sorted(fail_fast)),
        ),
        None,
    )


def _source_extension_closure_digest(
    *,
    init_symbol: str,
    objects: Sequence[_SourceExtensionObjectFact],
    runtime_symbols: Sequence[str],
) -> str:
    payload = {
        "schema_version": 1,
        "root_symbol": init_symbol,
        "objects": [
            {
                "source": str(fact.source_path),
                "object": fact.object_path.name,
                "source_sha256": fact.source_sha256,
                "object_sha256": fact.object_sha256,
                "defined_symbols": list(fact.defined_symbols),
                "undefined_symbols": list(fact.undefined_symbols),
            }
            for fact in objects
        ],
        "runtime_symbols": list(runtime_symbols),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _compute_source_extension_object_closure(
    *,
    init_symbol: str,
    object_facts: Sequence[_SourceExtensionObjectFact],
) -> tuple[_SourceExtensionObjectClosure | None, list[str]]:
    errors: list[str] = []
    owners: dict[str, list[_SourceExtensionObjectFact]] = {}
    for fact in object_facts:
        for symbol in fact.defined_symbols:
            owners.setdefault(symbol, []).append(fact)

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

    included: set[Path] = set()
    pending: list[_SourceExtensionObjectFact] = [init_owners[0]]
    runtime_symbols: set[str] = set()
    while pending:
        fact = pending.pop()
        if fact.object_path in included:
            continue
        included.add(fact.object_path)
        for symbol in fact.undefined_symbols:
            symbol_owners = owners.get(symbol)
            if not symbol_owners:
                runtime_symbols.add(symbol)
                continue
            if len(symbol_owners) != 1:
                owner_names = ", ".join(
                    owner.object_path.name for owner in symbol_owners
                )
                errors.append(
                    f"source extension symbol {symbol!r} is ambiguously defined by "
                    f"{owner_names}"
                )
                continue
            pending.append(symbol_owners[0])

    if errors:
        return None, errors

    closure_objects = tuple(
        fact for fact in object_facts if fact.object_path in included
    )
    runtime_symbols_sorted = tuple(sorted(runtime_symbols))
    closure_sha256 = _source_extension_closure_digest(
        init_symbol=init_symbol,
        objects=closure_objects,
        runtime_symbols=runtime_symbols_sorted,
    )
    return (
        _SourceExtensionObjectClosure(
            init_symbol=init_symbol,
            init_symbol_owner=init_owners[0],
            objects=closure_objects,
            runtime_symbols=runtime_symbols_sorted,
            closure_sha256=closure_sha256,
        ),
        [],
    )
