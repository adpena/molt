from __future__ import annotations

import functools
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shlex
from typing import Any, Mapping, cast

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.diagnostic_text import strip_terminal_decoration
from molt.cli.file_hashing import _sha256_file


_SCHEMA_VERSION = 3
_KIND = "molt_native_link_dependencies"
_FINGERPRINT_FIELDS = frozenset({"hash", "inputs_digest", "meta_digest", "rustc"})
_NATIVE_STATIC_LIBS_PREFIX = "native-static-libs:"


class NativeLinkDependencyManifestError(RuntimeError):
    """Native link dependencies are missing, corrupt, or artifact-mismatched."""


def native_link_dependency_manifest_path(runtime_lib: Path) -> Path:
    return runtime_lib.with_name(f"{runtime_lib.name}.native-link-deps.json")


def _strict_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def _strict_json_line(raw: str, *, context: str) -> Mapping[str, Any]:
    try:
        payload = json.loads(raw, object_pairs_hook=_strict_json_object)
    except (json.JSONDecodeError, ValueError) as exc:
        raise NativeLinkDependencyManifestError(
            f"invalid UTF-8 Cargo JSON for {context}: {exc}"
        ) from exc
    if not isinstance(payload, dict):
        raise NativeLinkDependencyManifestError(
            f"Cargo JSON for {context} must be an object"
        )
    return payload


def _strict_string_list(value: object, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise NativeLinkDependencyManifestError(f"{field} must be a string array")
    if any(not item for item in value):
        raise NativeLinkDependencyManifestError(f"{field} contains an empty value")
    return tuple(item for item in value if isinstance(item, str))


def _is_absolute_path(raw: str) -> bool:
    return PurePosixPath(raw).is_absolute() or PureWindowsPath(raw).is_absolute()


def _existing_directory(raw: str, *, field: str) -> Path:
    if not _is_absolute_path(raw):
        raise NativeLinkDependencyManifestError(f"{field} must be absolute")
    path = Path(raw)
    if not path.is_dir():
        raise NativeLinkDependencyManifestError(
            f"{field} does not name an existing directory: {raw}"
        )
    return path


def _linked_path_value(raw: str) -> str:
    if "=" not in raw:
        return raw
    kind, path = raw.split("=", 1)
    if not kind or not path:
        raise NativeLinkDependencyManifestError(
            f"invalid Cargo linked path directive {raw!r}"
        )
    return path


def _artifact_stat_identity(
    stat_result: os.stat_result,
) -> tuple[int, int, int, int, int]:
    return (
        stat_result.st_size,
        stat_result.st_mtime_ns,
        stat_result.st_ctime_ns,
        stat_result.st_dev,
        stat_result.st_ino,
    )


@functools.lru_cache(maxsize=256)
def _artifact_digest_for_identity(
    resolved_path: str, identity: tuple[int, int, int, int, int]
) -> str:
    path = Path(resolved_path)
    digest = _sha256_file(path)
    if _artifact_stat_identity(path.stat()) != identity:
        raise NativeLinkDependencyManifestError(
            f"runtime archive changed while hashing: {path}"
        )
    return digest


def _runtime_identity(runtime_lib: Path) -> dict[str, object]:
    try:
        resolved = runtime_lib.resolve(strict=True)
        before_identity = _artifact_stat_identity(resolved.stat())
    except OSError as exc:
        raise NativeLinkDependencyManifestError(
            f"cannot identify runtime archive {runtime_lib}: {exc}"
        ) from exc
    digest = _artifact_digest_for_identity(os.fspath(resolved), before_identity)
    after_identity = _artifact_stat_identity(resolved.stat())
    if before_identity != after_identity:
        raise NativeLinkDependencyManifestError(
            f"runtime archive changed while hashing: {resolved}"
        )
    return {"size_bytes": after_identity[0], "sha256": digest}


def _validated_source_fingerprint(value: object) -> dict[str, str | None]:
    if not isinstance(value, Mapping) or set(value) != _FINGERPRINT_FIELDS:
        raise NativeLinkDependencyManifestError(
            "source fingerprint must contain hash, inputs_digest, meta_digest, and rustc"
        )
    fingerprint = cast(Mapping[str, object], value)
    result: dict[str, str | None] = {}
    for field in sorted(_FINGERPRINT_FIELDS):
        raw = fingerprint[field]
        if field == "inputs_digest" and raw is None:
            result[field] = None
            continue
        if not isinstance(raw, str) or not raw:
            raise NativeLinkDependencyManifestError(
                f"source fingerprint {field} must be a non-empty string"
            )
        if field != "rustc" and re.fullmatch(r"[0-9a-f]{64}", raw) is None:
            raise NativeLinkDependencyManifestError(
                f"source fingerprint {field} must be a lowercase SHA-256 digest"
            )
        result[field] = raw
    return result


def _source_identity(
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> dict[str, object]:
    try:
        resolved_root = source_root.resolve(strict=True)
    except OSError as exc:
        raise NativeLinkDependencyManifestError(
            f"cannot resolve Cargo workspace root {source_root}: {exc}"
        ) from exc
    if not resolved_root.is_dir():
        raise NativeLinkDependencyManifestError(
            f"Cargo workspace root is not a directory: {resolved_root}"
        )
    # The absolute checkout is local provenance, not semantic identity. Persisting
    # it beside a shared/hydrated archive makes equivalent worktrees race to
    # rewrite the same sidecar. The complete source/config/toolchain fingerprint
    # is the executable authority; source_root is validated only as the invoking
    # workspace from which that fingerprint was computed.
    return {"fingerprint": _validated_source_fingerprint(source_fingerprint)}


def _native_static_lib_arguments(raw: str, *, target_triple: str | None) -> list[str]:
    target_is_windows = (
        "windows" in target_triple.lower() or "msvc" in target_triple.lower()
        if target_triple
        else os.name == "nt"
    )
    try:
        arguments = shlex.split(raw, posix=not target_is_windows)
    except ValueError as exc:
        raise NativeLinkDependencyManifestError(
            f"invalid rustc native-static-libs argument sequence: {exc}"
        ) from exc
    if target_is_windows:
        arguments = [
            argument[1:-1]
            if len(argument) >= 2
            and argument[0] == argument[-1]
            and argument[0] in {'"', "'"}
            else argument
            for argument in arguments
        ]
    if any(not argument for argument in arguments):
        raise NativeLinkDependencyManifestError(
            "rustc native-static-libs contains an empty argument"
        )
    return arguments


def manifest_from_cargo_json(
    cargo_stdout: str,
    *,
    cargo_stderr: str = "",
    runtime_lib: Path,
    cargo_profile: str,
    target_triple: str | None,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> dict[str, object]:
    """Capture exact build-script provenance from one successful Cargo JSON run."""
    scripts: list[dict[str, object]] = []
    native_static_lib_records: list[str] = []
    for line_number, raw in enumerate(cargo_stdout.splitlines(), start=1):
        if not raw:
            continue
        message = _strict_json_line(raw, context=f"Cargo stdout line {line_number}")
        reason = message.get("reason")
        if reason == "compiler-message":
            diagnostic = message.get("message")
            if isinstance(diagnostic, Mapping):
                diagnostic_message = diagnostic.get("message")
                if (
                    diagnostic.get("level") == "note"
                    and isinstance(diagnostic_message, str)
                    and diagnostic_message.startswith(_NATIVE_STATIC_LIBS_PREFIX)
                ):
                    native_static_lib_records.append(
                        diagnostic_message[len(_NATIVE_STATIC_LIBS_PREFIX) :].strip()
                    )
            continue
        if reason != "build-script-executed":
            continue
        package_id = message.get("package_id")
        out_dir = message.get("out_dir")
        if not isinstance(package_id, str) or not package_id:
            raise NativeLinkDependencyManifestError(
                "build-script-executed.package_id must be a non-empty string"
            )
        if not isinstance(out_dir, str) or not out_dir:
            raise NativeLinkDependencyManifestError(
                "build-script-executed.out_dir must be a non-empty string"
            )
        _existing_directory(out_dir, field="build-script-executed.out_dir")
        linked_paths = _strict_string_list(
            message.get("linked_paths"), field="linked_paths"
        )
        for linked_path in linked_paths:
            _existing_directory(
                _linked_path_value(linked_path),
                field="build-script-executed.linked_paths entry",
            )
        scripts.append(
            {
                "package_id": package_id,
                "out_dir": out_dir,
                "linked_paths": list(linked_paths),
                "linked_libs": list(
                    _strict_string_list(message.get("linked_libs"), field="linked_libs")
                ),
            }
        )
    for raw in cargo_stderr.splitlines():
        # Cargo/rustc diagnostics may still be decorated by a wrapper or an
        # externally supplied log even though Molt's own command requests
        # ``--color=never``. Terminal presentation is not semantic protocol
        # data, so normalize it at the one manifest-ingestion authority.
        raw = strip_terminal_decoration(raw)
        prefix = f"note: {_NATIVE_STATIC_LIBS_PREFIX}"
        if raw.startswith(prefix):
            native_static_lib_records.append(raw[len(prefix) :].strip())
    scripts.sort(key=lambda script: (str(script["package_id"]), str(script["out_dir"])))
    deduped_scripts: list[dict[str, object]] = []
    for script in scripts:
        identity = (str(script["package_id"]), str(script["out_dir"]))
        if deduped_scripts and identity == (
            str(deduped_scripts[-1]["package_id"]),
            str(deduped_scripts[-1]["out_dir"]),
        ):
            if script != deduped_scripts[-1]:
                raise NativeLinkDependencyManifestError(
                    "conflicting build-script-executed records for one package/out_dir"
                )
            continue
        deduped_scripts.append(script)
    if len(native_static_lib_records) != 1:
        raise NativeLinkDependencyManifestError(
            "Cargo rustc output must contain exactly one native-static-libs note"
        )
    native_static_libs_raw = native_static_lib_records[0]
    return {
        "schema_version": _SCHEMA_VERSION,
        "kind": _KIND,
        "runtime": _runtime_identity(runtime_lib),
        "source": _source_identity(source_root, source_fingerprint),
        "cargo": {
            "profile": cargo_profile,
            "profile_dir": runtime_lib.parent.name,
            "target_triple": target_triple,
        },
        "build_scripts": deduped_scripts,
        "native_static_libs": {
            "raw": native_static_libs_raw,
            "arguments": _native_static_lib_arguments(
                native_static_libs_raw,
                target_triple=target_triple,
            ),
        },
    }


def write_native_link_dependency_manifest(
    cargo_stdout: str,
    *,
    cargo_stderr: str = "",
    runtime_lib: Path,
    cargo_profile: str,
    target_triple: str | None,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> Path:
    manifest = manifest_from_cargo_json(
        cargo_stdout,
        cargo_stderr=cargo_stderr,
        runtime_lib=runtime_lib,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        source_root=source_root,
        source_fingerprint=source_fingerprint,
    )
    path = native_link_dependency_manifest_path(runtime_lib)
    _atomic_write_json(path, manifest, indent=2, sort_keys=True)
    return path


def _validated_build_scripts(value: object) -> tuple[Mapping[str, object], ...]:
    if not isinstance(value, list):
        raise NativeLinkDependencyManifestError("build_scripts must be an array")
    scripts: list[Mapping[str, object]] = []
    for index, script in enumerate(value):
        if not isinstance(script, dict) or set(script) != {
            "package_id",
            "out_dir",
            "linked_paths",
            "linked_libs",
        }:
            raise NativeLinkDependencyManifestError(
                f"build_scripts[{index}] has an invalid object shape"
            )
        script = cast(dict[str, object], script)
        package_id = script["package_id"]
        out_dir = script["out_dir"]
        if not isinstance(package_id, str) or not package_id:
            raise NativeLinkDependencyManifestError(
                f"build_scripts[{index}].package_id is invalid"
            )
        if not isinstance(out_dir, str) or not out_dir:
            raise NativeLinkDependencyManifestError(
                f"build_scripts[{index}].out_dir is invalid"
            )
        if not _is_absolute_path(out_dir):
            raise NativeLinkDependencyManifestError(
                f"build_scripts[{index}].out_dir must be absolute"
            )
        _existing_directory(out_dir, field=f"build_scripts[{index}].out_dir")
        linked_paths = _strict_string_list(script["linked_paths"], field="linked_paths")
        for linked_path in linked_paths:
            _existing_directory(
                _linked_path_value(linked_path),
                field=f"build_scripts[{index}].linked_paths entry",
            )
        linked_libs = _strict_string_list(script["linked_libs"], field="linked_libs")
        scripts.append(
            {
                "package_id": package_id,
                "out_dir": out_dir,
                "linked_paths": list(linked_paths),
                "linked_libs": list(linked_libs),
            }
        )
    identities = [
        (str(script["package_id"]), str(script["out_dir"])) for script in scripts
    ]
    if len(identities) != len(set(identities)):
        raise NativeLinkDependencyManifestError(
            "build_scripts contains a duplicate package/out_dir record"
        )
    if identities != sorted(identities):
        raise NativeLinkDependencyManifestError(
            "build_scripts provenance must be deterministically ordered"
        )
    return tuple(scripts)


def _validated_native_static_libs(
    value: object,
    *,
    target_triple: str | None,
) -> tuple[str, ...]:
    if not isinstance(value, dict) or set(value) != {"raw", "arguments"}:
        raise NativeLinkDependencyManifestError("invalid native_static_libs shape")
    raw = value.get("raw")
    arguments = value.get("arguments")
    if not isinstance(raw, str):
        raise NativeLinkDependencyManifestError("invalid native_static_libs payload")
    validated_arguments = _strict_string_list(
        arguments, field="native_static_libs.arguments"
    )
    if validated_arguments != tuple(
        _native_static_lib_arguments(raw, target_triple=target_triple)
    ):
        raise NativeLinkDependencyManifestError(
            "native_static_libs arguments do not match the rustc note"
        )
    return validated_arguments


def _read_native_link_dependency_manifest(
    runtime_lib: Path,
    *,
    target_triple: str | None,
    cargo_profile: str | None = None,
    source_root: Path | None = None,
    source_fingerprint: Mapping[str, object] | None = None,
) -> tuple[Mapping[str, object], tuple[Mapping[str, object], ...]]:
    path = native_link_dependency_manifest_path(runtime_lib)
    try:
        text = path.read_text(encoding="utf-8", errors="strict")
    except (OSError, UnicodeDecodeError) as exc:
        raise NativeLinkDependencyManifestError(
            f"cannot read native link dependency manifest {path}: {exc}"
        ) from exc
    manifest = _strict_json_line(text, context=str(path))
    if set(manifest) != {
        "schema_version",
        "kind",
        "runtime",
        "source",
        "cargo",
        "build_scripts",
        "native_static_libs",
    }:
        raise NativeLinkDependencyManifestError(f"invalid manifest shape: {path}")
    if (
        manifest.get("schema_version") != _SCHEMA_VERSION
        or manifest.get("kind") != _KIND
    ):
        raise NativeLinkDependencyManifestError(f"unsupported manifest schema: {path}")
    runtime = manifest.get("runtime")
    source = manifest.get("source")
    cargo = manifest.get("cargo")
    if not isinstance(runtime, dict) or set(runtime) != {"size_bytes", "sha256"}:
        raise NativeLinkDependencyManifestError(f"invalid runtime identity: {path}")
    if not isinstance(cargo, dict) or set(cargo) != {
        "profile",
        "profile_dir",
        "target_triple",
    }:
        raise NativeLinkDependencyManifestError(f"invalid Cargo identity: {path}")
    if not isinstance(source, dict) or set(source) != {"fingerprint"}:
        raise NativeLinkDependencyManifestError(f"invalid source identity: {path}")
    stored_fingerprint = _validated_source_fingerprint(source.get("fingerprint"))
    if source_root is not None:
        try:
            expected_root = source_root.resolve(strict=True)
        except OSError as exc:
            raise NativeLinkDependencyManifestError(
                f"cannot resolve expected source workspace {source_root}: {exc}"
            ) from exc
        if not expected_root.is_dir():
            raise NativeLinkDependencyManifestError(
                f"expected source workspace is not a directory: {expected_root}"
            )
    if source_fingerprint is not None:
        if stored_fingerprint != _validated_source_fingerprint(source_fingerprint):
            raise NativeLinkDependencyManifestError(
                f"native link manifest source fingerprint mismatch for {runtime_lib}"
            )
    if cargo.get("target_triple") != target_triple:
        raise NativeLinkDependencyManifestError(
            f"native link manifest target mismatch for {runtime_lib}"
        )
    expected_profile_dir = runtime_lib.parent.name
    if cargo.get("profile_dir") != expected_profile_dir:
        raise NativeLinkDependencyManifestError(
            f"native link manifest profile directory mismatch for {runtime_lib}"
        )
    if cargo_profile is not None and cargo.get("profile") != cargo_profile:
        raise NativeLinkDependencyManifestError(
            f"native link manifest Cargo profile mismatch for {runtime_lib}"
        )
    if not isinstance(cargo.get("profile"), str) or not cargo["profile"]:
        raise NativeLinkDependencyManifestError(f"invalid Cargo profile: {path}")
    expected_dir_for_profile = (
        "debug" if cargo["profile"] == "dev" else cargo["profile"]
    )
    if expected_dir_for_profile != cargo["profile_dir"]:
        raise NativeLinkDependencyManifestError(
            f"native link manifest Cargo profile identity is inconsistent for {runtime_lib}"
        )
    scripts = _validated_build_scripts(manifest.get("build_scripts"))
    _validated_native_static_libs(
        manifest.get("native_static_libs"), target_triple=target_triple
    )
    if runtime != _runtime_identity(runtime_lib):
        raise NativeLinkDependencyManifestError(
            f"native link manifest archive digest mismatch for {runtime_lib}"
        )
    return manifest, scripts


def read_native_link_dependency_manifest(
    runtime_lib: Path,
    *,
    target_triple: str | None,
    cargo_profile: str | None = None,
    source_root: Path | None = None,
    source_fingerprint: Mapping[str, object] | None = None,
) -> Mapping[str, object]:
    manifest, _scripts = _read_native_link_dependency_manifest(
        runtime_lib,
        target_triple=target_triple,
        cargo_profile=cargo_profile,
        source_root=source_root,
        source_fingerprint=source_fingerprint,
    )
    return manifest


def _directive_parts(raw: str) -> tuple[str | None, str]:
    if "=" not in raw:
        return None, raw
    kind, value = raw.split("=", 1)
    if not kind or not value:
        raise NativeLinkDependencyManifestError(f"invalid Cargo link directive {raw!r}")
    return kind, value


def _native_search_directories(
    scripts: tuple[Mapping[str, object], ...],
) -> tuple[tuple[Path, ...], tuple[Path, ...]]:
    native_dirs: set[Path] = set()
    framework_dirs: set[Path] = set()
    for script in scripts:
        for raw in _strict_string_list(script["linked_paths"], field="linked_paths"):
            kind, path = _directive_parts(raw)
            if kind not in {None, "all", "crate", "dependency", "framework", "native"}:
                raise NativeLinkDependencyManifestError(
                    f"unsupported Cargo linked_paths kind {kind!r}"
                )
            if not _is_absolute_path(path):
                raise NativeLinkDependencyManifestError(
                    f"Cargo linked path must be absolute: {path!r}"
                )
            if kind == "framework":
                framework_dirs.add(Path(path))
            else:
                native_dirs.add(Path(path))
    return (
        tuple(sorted(native_dirs, key=os.fspath)),
        tuple(sorted(framework_dirs, key=os.fspath)),
    )


def _library_candidate_names(argument: str, *, object_format: str) -> tuple[str, ...]:
    if argument.startswith("-l:"):
        return (argument[3:],)
    if argument.startswith("-l") and len(argument) > 2:
        name = argument[2:]
        if object_format == "coff":
            return (f"{name}.lib", f"lib{name}.a")
        if object_format == "macho":
            return (f"lib{name}.a", f"lib{name}.dylib", f"lib{name}.tbd")
        return (f"lib{name}.a", f"lib{name}.so")
    if object_format == "coff" and argument.lower().endswith(".lib"):
        return (argument,)
    return ()


def _unique_search_match(
    candidate_names: tuple[str, ...],
    search_dirs: tuple[Path, ...],
    *,
    context: str,
) -> Path | None:
    matches = {
        candidate.resolve()
        for directory in search_dirs
        for candidate_name in candidate_names
        if (candidate := directory / candidate_name).is_file()
        or (candidate.suffix == ".framework" and candidate.is_dir())
    }
    if len(matches) > 1:
        raise NativeLinkDependencyManifestError(
            f"ambiguous Cargo native library custody for {context!r}: "
            + ", ".join(sorted(map(os.fspath, matches)))
        )
    return next(iter(matches), None)


def _native_link_flags(
    arguments: tuple[str, ...],
    scripts: tuple[Mapping[str, object], ...],
    *,
    object_format: str,
) -> list[str]:
    """Replay rustc's exact native-static-libs sequence with explicit custody."""
    if object_format not in {"coff", "elf", "macho"}:
        raise NativeLinkDependencyManifestError(
            f"unsupported native object format {object_format!r}"
        )
    native_dirs, framework_dirs = _native_search_directories(scripts)
    flags: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument in {"-framework", "-weak_framework"}:
            if object_format != "macho" or index + 1 >= len(arguments):
                raise NativeLinkDependencyManifestError(
                    f"invalid rustc framework argument sequence at {argument!r}"
                )
            framework = arguments[index + 1]
            match = _unique_search_match(
                (f"{framework}.framework",),
                framework_dirs,
                context=framework,
            )
            if match is not None:
                flags.append(f"-F{match.parent}")
            flags.extend((argument, framework))
            index += 2
            continue
        path = Path(argument)
        if path.is_absolute():
            if not path.is_file():
                raise NativeLinkDependencyManifestError(
                    f"rustc native-static-libs path no longer exists: {path}"
                )
            flags.append(os.fspath(path))
            index += 1
            continue
        candidates = _library_candidate_names(argument, object_format=object_format)
        match = _unique_search_match(candidates, native_dirs, context=argument)
        if match is not None:
            if object_format == "coff" and not argument.startswith("-l"):
                flags.append(os.fspath(match))
            else:
                flags.extend((f"-L{match.parent}", argument))
        elif object_format == "coff" and (
            argument.lower().endswith(".lib") or argument.startswith("/")
        ):
            # rustc prints linker arguments, while Molt executes a Clang driver.
            # Preserve the exact ordered token (including duplicates) behind the
            # driver's transparent linker forwarding syntax so Clang does not
            # misclassify a system .lib or /defaultlib option as an input path.
            flags.append(f"-Wl,{argument}")
        else:
            flags.append(argument)
        index += 1
    return flags


def native_link_flags_from_manifest(
    manifest: Mapping[str, object],
    *,
    object_format: str,
) -> list[str]:
    cargo = manifest.get("cargo")
    if not isinstance(cargo, Mapping):
        raise NativeLinkDependencyManifestError("manifest has no Cargo identity")
    target_triple = cargo.get("target_triple")
    if target_triple is not None and not isinstance(target_triple, str):
        raise NativeLinkDependencyManifestError("manifest target triple is invalid")
    return _native_link_flags(
        _validated_native_static_libs(
            manifest.get("native_static_libs"), target_triple=target_triple
        ),
        _validated_build_scripts(manifest.get("build_scripts")),
        object_format=object_format,
    )


def read_native_link_flags(
    runtime_lib: Path,
    *,
    target_triple: str | None,
    object_format: str,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> list[str]:
    manifest, scripts = _read_native_link_dependency_manifest(
        runtime_lib,
        target_triple=target_triple,
        source_root=source_root,
        source_fingerprint=source_fingerprint,
    )
    arguments = _validated_native_static_libs(
        manifest.get("native_static_libs"), target_triple=target_triple
    )
    return _native_link_flags(arguments, scripts, object_format=object_format)
