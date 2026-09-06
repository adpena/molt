from __future__ import annotations

import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shlex
from typing import Any, Mapping, cast

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.diagnostic_text import strip_terminal_decoration
from molt.cli.native_link_custody import (
    NativeLinkCustodyEntry,
    NativeLinkCustodyError,
    ensure_native_link_custody,
    publish_native_link_custody,
    validate_native_link_custody,
)
from molt.cli.native_link_plan import resolve_native_target_spec
from molt.cli.runtime_artifact_selection import RUNTIME_STATICLIB_ARTIFACTS
from molt.cli.runtime_build_identity import (
    RuntimeBuildIdentity,
    require_native_runtime_staticlib_identity,
)
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
    validate_artifact_content_identity,
)
from molt.exact_json import loads_exact, read_exact


_SCHEMA_VERSION = 5
_KIND = "molt_native_link_dependencies"
_NATIVE_STATIC_LIBS_PREFIX = "native-static-libs:"
_LINK_PLAN_SCHEMA = "molt.native-link-plan.v1"
_FRAMEWORK_NAME_RE = re.compile(r"[A-Za-z0-9_+.-]+")
_DYNAMIC_LIBRARY_RE = re.compile(
    r"(?:.*\.so(?:\.[0-9]+)*|.*\.(?:dylib|dll|tbd)|.*\.dll\.a)",
    re.I,
)
_CUSTODIABLE_LINK_FILE_SUFFIXES = frozenset(
    {".a", ".lib", ".o", ".obj", ".res", ".rlib"}
)
_COFF_PATH_OPTION_PREFIXES = (
    "/def:",
    "/implib:",
    "/libpath:",
    "/lldmap:",
    "/manifestfile:",
    "/manifestinput:",
    "/natvis:",
    "/order:",
    "/out:",
    "/pdb:",
    "/pdbaltpath:",
    "/stub:",
    "/wholearchive:",
    "/winsysroot:",
)
_PATH_BEARING_LINKER_PREFIXES = (
    "-F",
    "-L",
    "-T",
    "--dynamic-list",
    "--just-symbols",
    "--retain-symbols-file",
    "--script",
    "--sysroot",
    "--version-script",
    "-bundle_loader",
    "-filelist",
    "-force_load",
    "-fuse-ld",
    "-isysroot",
    "-order_file",
    "-reexport_library",
    "-weak_library",
    "-Wl,@",
    "-Wl,-T",
    "-Wl,-bundle_loader",
    "-Wl,-filelist",
    "-Wl,-force_load",
    "-Wl,-framework",
    "-Wl,-l",
    "-Wl,-order_file",
    "-Wl,-rpath",
    "-Wl,-rpath-link",
    "-Wl,-reexport_library",
    "-Wl,-syslibroot",
    "-Wl,-weak_framework",
    "-Wl,-weak_library",
    "-Wl,--dynamic-list",
    "-Wl,--just-symbols",
    "-Wl,--retain-symbols-file",
    "-Wl,--script",
    "-Wl,--sysroot",
    "-Wl,--version-script",
)


class NativeLinkDependencyManifestError(RuntimeError):
    """Native link dependencies are missing, corrupt, or artifact-mismatched."""


def native_link_dependency_manifest_path(runtime_lib: Path) -> Path:
    return runtime_lib.with_name(f"{runtime_lib.name}.native-link-deps.json")


def _strict_json_line(raw: str, *, context: str) -> Mapping[str, Any]:
    try:
        payload = loads_exact(raw)
    except ValueError as exc:
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


def _runtime_identity(runtime_lib: Path) -> dict[str, object]:
    try:
        return artifact_content_identity(runtime_lib)
    except StaticArchiveIdentityError as exc:
        raise NativeLinkDependencyManifestError(
            f"cannot identify runtime archive {runtime_lib}: {exc}"
        ) from exc


def _validated_native_runtime_build_identity(
    value: object,
    *,
    cargo_profile: str,
    target_triple: str | None,
) -> RuntimeBuildIdentity:
    try:
        return require_native_runtime_staticlib_identity(
            value,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
        )
    except (TypeError, ValueError) as exc:
        raise NativeLinkDependencyManifestError(
            "runtime build identity is not the selected native staticlib identity: "
            f"{exc}"
        ) from exc


def _native_static_lib_arguments(raw: str, *, object_format: str) -> list[str]:
    target_is_windows = object_format == "coff"
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


def _object_format_for_identity(runtime_build_identity: RuntimeBuildIdentity) -> str:
    try:
        return resolve_native_target_spec(
            runtime_build_identity.effective_target
        ).object_format.value
    except RuntimeError as exc:
        raise NativeLinkDependencyManifestError(str(exc)) from exc


def _require_object_format(
    runtime_build_identity: RuntimeBuildIdentity, object_format: str
) -> None:
    expected = _object_format_for_identity(runtime_build_identity)
    if object_format != expected:
        raise NativeLinkDependencyManifestError(
            "native link object format does not match captured runtime target: "
            f"expected {expected!r}, got {object_format!r}"
        )


def _is_dynamic_library(path: Path) -> bool:
    return _DYNAMIC_LIBRARY_RE.fullmatch(path.name) is not None


def _is_custodiable_link_file(path: Path) -> bool:
    return path.suffix.casefold() in _CUSTODIABLE_LINK_FILE_SUFFIXES


def _reject_path_bearing_argument(argument: str, *, object_format: str) -> None:
    if not argument or "\x00" in argument:
        raise NativeLinkDependencyManifestError(
            "native-static-libs contains an invalid argument"
        )
    lowered = argument.casefold()
    if argument.startswith("@") or argument.startswith(_PATH_BEARING_LINKER_PREFIXES):
        raise NativeLinkDependencyManifestError(
            "native-static-libs contains an unowned path-bearing argument: "
            f"{argument!r}"
        )
    if object_format == "coff" and argument.startswith("/"):
        if (
            lowered.startswith(_COFF_PATH_OPTION_PREFIXES)
            or "/" in argument[1:]
            or "\\" in argument
        ):
            raise NativeLinkDependencyManifestError(
                "native-static-libs contains an unowned path-bearing argument: "
                f"{argument!r}"
            )
        return
    if (
        _is_absolute_path(argument)
        or re.search(r"(?i)[a-z]:[\\/]", argument)
        or re.search(r"(?:^|[=,@])/[A-Za-z0-9_.-]", argument)
    ):
        raise NativeLinkDependencyManifestError(
            "native-static-libs contains an unowned path-bearing argument: "
            f"{argument!r}"
        )


def _looks_like_relative_link_input(argument: str) -> bool:
    lowered = argument.casefold()
    return (
        lowered.endswith(
            (".a", ".lib", ".o", ".obj", ".rlib", ".dylib", ".dll", ".tbd")
        )
        or ".so." in lowered
        or lowered.endswith(".so")
    )


def _library_name_keys(value: str, *, object_format: str) -> frozenset[str]:
    name = value
    if name.startswith("-l:"):
        name = name[3:]
    elif name.startswith("-l") and len(name) > 2:
        name = name[2:]
    normalized = name.casefold() if object_format == "coff" else name
    keys = {normalized}
    lowered = normalized.casefold()
    for suffix in (".lib", ".a", ".dylib", ".tbd", ".so"):
        if lowered.endswith(suffix):
            stem = normalized[: -len(suffix)]
            keys.add(stem)
            if stem.casefold().startswith("lib") and len(stem) > 3:
                keys.add(stem[3:])
            break
    return frozenset(keys)


def _declared_library_policy(
    raw: str,
    *,
    object_format: str,
) -> tuple[str, frozenset[str]] | None:
    kind, name = _directive_parts(raw)
    base_kind = kind.split(":", 1)[0] if kind is not None else "dylib"
    if base_kind in {"framework", "weak_framework"}:
        return None
    if base_kind in {"dylib", "raw-dylib"}:
        policy = "dynamic"
    elif base_kind in {"static", "static-nobundle"}:
        policy = "static"
    else:
        raise NativeLinkDependencyManifestError(
            f"unsupported Cargo linked library kind {base_kind!r}"
        )
    return policy, _library_name_keys(name, object_format=object_format)


def _relocatable_link_plan(
    arguments: tuple[str, ...],
    *,
    runtime_lib: Path,
    object_format: str,
    native_dirs: tuple[Path, ...],
    framework_dirs: tuple[Path, ...],
    declared_dynamic_libraries: frozenset[str],
    declared_static_libraries: frozenset[str],
) -> tuple[dict[str, object], dict[str, object]]:
    pending: list[dict[str, object]] = []
    custody_sources: list[Path] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument in {"-framework", "-weak_framework"}:
            if object_format != "macho" or index + 1 >= len(arguments):
                raise NativeLinkDependencyManifestError(
                    f"invalid rustc framework argument sequence at {argument!r}"
                )
            framework = arguments[index + 1]
            if _FRAMEWORK_NAME_RE.fullmatch(framework) is None:
                raise NativeLinkDependencyManifestError(
                    f"invalid native framework name: {framework!r}"
                )
            match = _unique_search_match(
                (f"{framework}.framework",),
                framework_dirs,
                context=framework,
            )
            if match is not None:
                raise NativeLinkDependencyManifestError(
                    "non-system frameworks are not relocatable native-link inputs; "
                    "package the framework and its runtime loader contract explicitly: "
                    f"{framework}"
                )
            pending.append(
                {
                    "kind": "system-framework",
                    "name": framework,
                    "weak": argument == "-weak_framework",
                }
            )
            index += 2
            continue

        if _is_absolute_path(argument) and not (
            object_format == "coff" and argument.startswith("/")
        ):
            source = Path(argument)
            if not source.is_file():
                raise NativeLinkDependencyManifestError(
                    f"rustc native-static-libs path no longer exists: {argument}"
                )
            if _is_dynamic_library(source):
                raise NativeLinkDependencyManifestError(
                    "non-system dynamic libraries require an explicit runtime "
                    f"distribution contract: {source.name}"
                )
            if not _is_custodiable_link_file(source):
                raise NativeLinkDependencyManifestError(
                    "native link input has no supported relocatable file role: "
                    f"{source.name}"
                )
            if object_format == "coff" and source.suffix.casefold() == ".lib":
                raise NativeLinkDependencyManifestError(
                    "direct COFF .lib input is ambiguous between a static archive and "
                    "a dynamic import library; use a declared static library search "
                    f"input or provide an explicit runtime distribution contract: {source.name}"
                )
            resolved = source.resolve()
            custody_sources.append(resolved)
            pending.append({"kind": "custodied-file", "_source": resolved})
            index += 1
            continue

        candidates = _library_candidate_names(argument, object_format=object_format)
        match = _unique_search_match(candidates, native_dirs, context=argument)
        if match is not None:
            library_keys = _library_name_keys(argument, object_format=object_format)
            if _is_dynamic_library(match) or (
                object_format == "coff"
                and bool(library_keys & declared_dynamic_libraries)
            ):
                raise NativeLinkDependencyManifestError(
                    "non-system dynamic libraries require an explicit runtime "
                    f"distribution contract: {match.name}"
                )
            if (
                object_format == "coff"
                and match.suffix.casefold() == ".lib"
                and not library_keys & declared_static_libraries
            ):
                raise NativeLinkDependencyManifestError(
                    "local COFF .lib input is ambiguous between a static archive and "
                    "a dynamic import library; declare static custody or provide an "
                    f"explicit runtime distribution contract: {match.name}"
                )
            custody_sources.append(match)
            pending.append(
                {
                    "kind": "custodied-library",
                    "argument": argument,
                    "_source": match,
                }
            )
        elif candidates:
            _reject_path_bearing_argument(argument, object_format=object_format)
            pending.append({"kind": "system-library", "argument": argument})
        else:
            if _looks_like_relative_link_input(argument):
                raise NativeLinkDependencyManifestError(
                    "relative native link inputs have no relocatable custody: "
                    f"{argument!r}"
                )
            _reject_path_bearing_argument(argument, object_format=object_format)
            pending.append({"kind": "linker-argument", "argument": argument})
        index += 1

    try:
        custody, source_ids = publish_native_link_custody(
            runtime_lib,
            custody_sources,
        )
    except NativeLinkCustodyError as exc:
        raise NativeLinkDependencyManifestError(str(exc)) from exc
    items: list[dict[str, object]] = []
    for pending_item in pending:
        source = pending_item.pop("_source", None)
        if isinstance(source, Path):
            identifier = source_ids.get(source.absolute())
            if identifier is None:
                raise NativeLinkDependencyManifestError(
                    f"native link input escaped custody publication: {source}"
                )
            pending_item["entry_id"] = identifier
        items.append(pending_item)
    return {"schema": _LINK_PLAN_SCHEMA, "items": items}, custody


def manifest_from_cargo_json(
    cargo_stdout: str,
    *,
    cargo_stderr: str = "",
    runtime_lib: Path,
    cargo_profile: str,
    target_triple: str | None,
    runtime_build_identity: RuntimeBuildIdentity,
) -> dict[str, object]:
    """Capture exact build-script provenance from one successful Cargo JSON run."""
    runtime_build_identity = _validated_native_runtime_build_identity(
        runtime_build_identity,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
    )
    object_format = _object_format_for_identity(runtime_build_identity)
    script_records: dict[tuple[str, str], tuple[tuple[str, ...], tuple[str, ...]]] = {}
    native_dirs: set[Path] = set()
    framework_dirs: set[Path] = set()
    declared_dynamic_libraries: set[str] = set()
    declared_static_libraries: set[str] = set()
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
        linked_libs = _strict_string_list(
            message.get("linked_libs"), field="linked_libs"
        )
        identity = (package_id, out_dir)
        record = (linked_paths, linked_libs)
        existing = script_records.get(identity)
        if existing is not None and existing != record:
            raise NativeLinkDependencyManifestError(
                "conflicting build-script-executed records for one package/out_dir"
            )
        script_records[identity] = record
        for linked_lib in linked_libs:
            policy = _declared_library_policy(
                linked_lib,
                object_format=object_format,
            )
            if policy is None:
                continue
            policy_name, names = policy
            if policy_name == "dynamic":
                declared_dynamic_libraries.update(names)
            else:
                declared_static_libraries.update(names)
        for linked_path in linked_paths:
            kind, raw_path = _directive_parts(linked_path)
            if kind not in {None, "all", "crate", "dependency", "framework", "native"}:
                raise NativeLinkDependencyManifestError(
                    f"unsupported Cargo linked_paths kind {kind!r}"
                )
            directory = _existing_directory(
                raw_path,
                field="build-script-executed.linked_paths entry",
            ).resolve()
            if kind == "framework":
                framework_dirs.add(directory)
            else:
                native_dirs.add(directory)
    for raw in cargo_stderr.splitlines():
        # Cargo/rustc diagnostics may still be decorated by a wrapper or an
        # externally supplied log even though Molt's own command requests
        # ``--color=never``. Terminal presentation is not semantic protocol
        # data, so normalize it at the one manifest-ingestion authority.
        raw = strip_terminal_decoration(raw)
        prefix = f"note: {_NATIVE_STATIC_LIBS_PREFIX}"
        if raw.startswith(prefix):
            native_static_lib_records.append(raw[len(prefix) :].strip())
    if len(native_static_lib_records) != 1:
        raise NativeLinkDependencyManifestError(
            "Cargo rustc output must contain exactly one native-static-libs note"
        )
    native_static_libs_raw = native_static_lib_records[0]
    arguments = tuple(
        _native_static_lib_arguments(
            native_static_libs_raw,
            object_format=object_format,
        )
    )
    link_plan, custody = _relocatable_link_plan(
        arguments,
        runtime_lib=runtime_lib,
        object_format=object_format,
        native_dirs=tuple(sorted(native_dirs, key=os.fspath)),
        framework_dirs=tuple(sorted(framework_dirs, key=os.fspath)),
        declared_dynamic_libraries=frozenset(declared_dynamic_libraries),
        declared_static_libraries=frozenset(declared_static_libraries),
    )
    return {
        "schema_version": _SCHEMA_VERSION,
        "kind": _KIND,
        "runtime": _runtime_identity(runtime_lib),
        "runtime_build_identity": runtime_build_identity.to_dict(),
        "cargo": {
            "profile": cargo_profile,
            "profile_dir": runtime_lib.parent.name,
            "target_triple": target_triple,
        },
        "link_plan": link_plan,
        "custody": custody,
    }


def write_native_link_dependency_manifest(
    cargo_stdout: str,
    *,
    cargo_stderr: str = "",
    runtime_lib: Path,
    cargo_profile: str,
    target_triple: str | None,
    runtime_build_identity: RuntimeBuildIdentity,
) -> Path:
    manifest = manifest_from_cargo_json(
        cargo_stdout,
        cargo_stderr=cargo_stderr,
        runtime_lib=runtime_lib,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        runtime_build_identity=runtime_build_identity,
    )
    path = native_link_dependency_manifest_path(runtime_lib)
    _atomic_write_json(path, manifest, indent=2, sort_keys=True)
    return path


def validate_native_link_dependency_manifest(
    manifest: Mapping[str, object],
    *,
    runtime_identity: Mapping[str, object],
    context: str,
    target_triple: str | None,
    cargo_profile: str | None = None,
    runtime_build_identity: RuntimeBuildIdentity | None = None,
) -> tuple[Mapping[str, object], tuple[Mapping[str, object], ...]]:
    """Validate one decoded manifest against its artifact and build authorities."""
    if set(manifest) != {
        "schema_version",
        "kind",
        "runtime",
        "runtime_build_identity",
        "cargo",
        "link_plan",
        "custody",
    }:
        raise NativeLinkDependencyManifestError(f"invalid manifest shape: {context}")
    if (
        type(manifest.get("schema_version")) is not int
        or manifest.get("schema_version") != _SCHEMA_VERSION
        or manifest.get("kind") != _KIND
    ):
        raise NativeLinkDependencyManifestError(
            f"unsupported manifest schema: {context}"
        )
    runtime = manifest.get("runtime")
    stored_build_identity_value = manifest.get("runtime_build_identity")
    cargo = manifest.get("cargo")
    try:
        runtime = validate_artifact_content_identity(runtime)
        runtime_identity = validate_artifact_content_identity(runtime_identity)
    except StaticArchiveIdentityError as exc:
        raise NativeLinkDependencyManifestError(
            f"invalid runtime identity: {context}: {exc}"
        ) from exc
    if not isinstance(cargo, dict) or set(cargo) != {
        "profile",
        "profile_dir",
        "target_triple",
    }:
        raise NativeLinkDependencyManifestError(f"invalid Cargo identity: {context}")
    cargo = cast(dict[str, object], cargo)
    cargo_profile_value = cargo.get("profile")
    if not isinstance(cargo_profile_value, str) or not cargo_profile_value:
        raise NativeLinkDependencyManifestError(f"invalid Cargo profile: {context}")
    if cargo.get("target_triple") != target_triple:
        raise NativeLinkDependencyManifestError(
            f"native link manifest target mismatch: {context}"
        )
    if cargo_profile is not None and cargo.get("profile") != cargo_profile:
        raise NativeLinkDependencyManifestError(
            f"native link manifest Cargo profile mismatch: {context}"
        )
    stored_build_identity = _validated_native_runtime_build_identity(
        stored_build_identity_value,
        cargo_profile=cargo_profile_value,
        target_triple=target_triple,
    )
    if (
        runtime_build_identity is not None
        and stored_build_identity != runtime_build_identity
    ):
        raise NativeLinkDependencyManifestError(
            f"native link manifest runtime build identity mismatch: {context}"
        )
    expected_dir_for_profile = (
        "debug" if cargo["profile"] == "dev" else cargo["profile"]
    )
    if expected_dir_for_profile != cargo["profile_dir"]:
        raise NativeLinkDependencyManifestError(
            f"native link manifest Cargo profile identity is inconsistent: {context}"
        )
    try:
        _custody, entries = validate_native_link_custody(
            manifest.get("custody"),
            context=context,
        )
    except NativeLinkCustodyError as exc:
        raise NativeLinkDependencyManifestError(str(exc)) from exc
    link_items = _validated_link_plan(
        manifest.get("link_plan"),
        entries=entries,
        object_format=_object_format_for_identity(stored_build_identity),
        context=context,
    )
    if runtime != runtime_identity:
        raise NativeLinkDependencyManifestError(
            f"native link manifest archive digest mismatch: {context}"
        )
    return manifest, link_items


def read_native_link_dependency_manifest_payload(path: Path) -> Mapping[str, object]:
    """Read one bounded, stable direct-file generation of native metadata."""
    try:
        payload = read_exact(
            path,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="native link dependency manifest",
        )
    except (OSError, ValueError) as exc:
        raise NativeLinkDependencyManifestError(
            f"cannot read native link dependency manifest {path}: {exc}"
        ) from exc
    if not isinstance(payload, dict):
        raise NativeLinkDependencyManifestError(
            f"native link dependency manifest must be an object: {path}"
        )
    return payload


def _read_native_link_dependency_manifest(
    runtime_lib: Path,
    *,
    target_triple: str | None,
    cargo_profile: str | None = None,
    runtime_build_identity: RuntimeBuildIdentity | None = None,
) -> tuple[Mapping[str, object], tuple[Mapping[str, object], ...]]:
    path = native_link_dependency_manifest_path(runtime_lib)
    manifest = read_native_link_dependency_manifest_payload(path)
    return validate_native_link_dependency_manifest(
        manifest,
        runtime_identity=_runtime_identity(runtime_lib),
        context=str(path),
        target_triple=target_triple,
        cargo_profile=cargo_profile,
        runtime_build_identity=runtime_build_identity,
    )


def read_native_link_dependency_manifest(
    runtime_lib: Path,
    *,
    target_triple: str | None,
    cargo_profile: str | None = None,
    runtime_build_identity: RuntimeBuildIdentity | None = None,
) -> Mapping[str, object]:
    manifest, _scripts = _read_native_link_dependency_manifest(
        runtime_lib,
        target_triple=target_triple,
        cargo_profile=cargo_profile,
        runtime_build_identity=runtime_build_identity,
    )
    try:
        custody, _entries = validate_native_link_custody(
            manifest.get("custody"),
            context=str(runtime_lib),
        )
        ensure_native_link_custody(runtime_lib, custody)
    except NativeLinkCustodyError as exc:
        raise NativeLinkDependencyManifestError(
            f"native link custody archive is unavailable or invalid: {exc}"
        ) from exc
    return manifest


def _directive_parts(raw: str) -> tuple[str | None, str]:
    if "=" not in raw:
        return None, raw
    kind, value = raw.split("=", 1)
    if not kind or not value:
        raise NativeLinkDependencyManifestError(f"invalid Cargo link directive {raw!r}")
    return kind, value


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
    if (
        object_format == "coff"
        and not argument.startswith("/")
        and argument.lower().endswith(".lib")
    ):
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


def _validated_link_plan(
    value: object,
    *,
    entries: tuple[NativeLinkCustodyEntry, ...],
    object_format: str,
    context: str,
) -> tuple[Mapping[str, object], ...]:
    if object_format not in {"coff", "elf", "macho"}:
        raise NativeLinkDependencyManifestError(
            f"unsupported native object format {object_format!r}"
        )
    if not isinstance(value, dict) or set(value) != {"schema", "items"}:
        raise NativeLinkDependencyManifestError(f"invalid native link plan: {context}")
    if value.get("schema") != _LINK_PLAN_SCHEMA:
        raise NativeLinkDependencyManifestError(
            f"unsupported native link plan schema: {context}"
        )
    raw_items = value.get("items")
    if not isinstance(raw_items, list):
        raise NativeLinkDependencyManifestError(
            f"native link plan items must be an array: {context}"
        )
    entries_by_id = {entry.identifier: entry for entry in entries}
    used_entries: set[str] = set()
    items: list[Mapping[str, object]] = []
    for index, item in enumerate(raw_items):
        item_context = f"{context}:link_plan.items[{index}]"
        if not isinstance(item, dict):
            raise NativeLinkDependencyManifestError(
                f"native link plan item must be an object: {item_context}"
            )
        item = cast(dict[str, object], item)
        kind = item.get("kind")
        if not isinstance(kind, str):
            raise NativeLinkDependencyManifestError(
                f"native link plan item kind must be a string: {item_context}"
            )
        if kind in {"system-library", "linker-argument"}:
            if set(item) != {"kind", "argument"}:
                raise NativeLinkDependencyManifestError(
                    f"invalid {kind} item: {item_context}"
                )
            argument = item.get("argument")
            if not isinstance(argument, str):
                raise NativeLinkDependencyManifestError(
                    f"invalid native link argument: {item_context}"
                )
            _reject_path_bearing_argument(argument, object_format=object_format)
            candidates = _library_candidate_names(argument, object_format=object_format)
            if kind == "system-library" and not candidates:
                raise NativeLinkDependencyManifestError(
                    f"system library item is not a library selector: {item_context}"
                )
            if kind == "linker-argument" and (
                candidates or _looks_like_relative_link_input(argument)
            ):
                raise NativeLinkDependencyManifestError(
                    f"linker argument bypasses native input custody: {item_context}"
                )
        elif kind == "custodied-library":
            if set(item) != {"kind", "argument", "entry_id"}:
                raise NativeLinkDependencyManifestError(
                    f"invalid custodied library item: {item_context}"
                )
            argument = item.get("argument")
            entry_id = item.get("entry_id")
            if not isinstance(argument, str) or not isinstance(entry_id, str):
                raise NativeLinkDependencyManifestError(
                    f"invalid custodied library item: {item_context}"
                )
            _reject_path_bearing_argument(argument, object_format=object_format)
            entry = entries_by_id.get(entry_id)
            candidates = _library_candidate_names(argument, object_format=object_format)
            if (
                entry is None
                or entry.filename not in candidates
                or _is_dynamic_library(Path(entry.filename))
            ):
                raise NativeLinkDependencyManifestError(
                    f"custodied library does not match its selector: {item_context}"
                )
            assert isinstance(entry_id, str)
            used_entries.add(entry_id)
        elif kind == "custodied-file":
            if set(item) != {"kind", "entry_id"}:
                raise NativeLinkDependencyManifestError(
                    f"invalid custodied file item: {item_context}"
                )
            entry_id = item.get("entry_id")
            entry = entries_by_id.get(entry_id) if isinstance(entry_id, str) else None
            if (
                entry is None
                or _is_dynamic_library(Path(entry.filename))
                or not _is_custodiable_link_file(Path(entry.filename))
            ):
                raise NativeLinkDependencyManifestError(
                    f"custodied file has no matching entry: {item_context}"
                )
            assert isinstance(entry_id, str)
            used_entries.add(entry_id)
        elif kind == "system-framework":
            if set(item) != {"kind", "name", "weak"} or object_format != "macho":
                raise NativeLinkDependencyManifestError(
                    f"invalid system framework item: {item_context}"
                )
            name = item.get("name")
            weak = item.get("weak")
            if (
                not isinstance(name, str)
                or _FRAMEWORK_NAME_RE.fullmatch(name) is None
                or not isinstance(weak, bool)
            ):
                raise NativeLinkDependencyManifestError(
                    f"invalid system framework item: {item_context}"
                )
        else:
            raise NativeLinkDependencyManifestError(
                f"unsupported native link plan item kind {kind!r}: {item_context}"
            )
        items.append(item)
    if used_entries != set(entries_by_id):
        missing = sorted(set(entries_by_id) - used_entries)
        raise NativeLinkDependencyManifestError(
            f"native-link custody contains unreferenced entries: {missing}"
        )
    return tuple(items)


def _native_link_flags(
    items: tuple[Mapping[str, object], ...],
    *,
    object_format: str,
    custody_paths: Mapping[str, Path],
) -> list[str]:
    """Render the path-neutral Cargo link plan through local content custody."""
    if object_format not in {"coff", "elf", "macho"}:
        raise NativeLinkDependencyManifestError(
            f"unsupported native object format {object_format!r}"
        )
    flags: list[str] = []
    for item in items:
        kind = item["kind"]
        if kind == "system-framework":
            flags.extend(
                (
                    "-weak_framework" if item["weak"] else "-framework",
                    str(item["name"]),
                )
            )
            continue
        if kind == "custodied-file":
            path = custody_paths.get(str(item["entry_id"]))
            if path is None:
                raise NativeLinkDependencyManifestError(
                    f"native link file is absent from custody: {item['entry_id']}"
                )
            flags.append(os.fspath(path))
            continue
        argument = str(item["argument"])
        if kind == "custodied-library":
            path = custody_paths.get(str(item["entry_id"]))
            if path is None:
                raise NativeLinkDependencyManifestError(
                    f"native link library is absent from custody: {item['entry_id']}"
                )
            if object_format == "coff" and not argument.startswith("-l"):
                flags.append(os.fspath(path))
            else:
                flags.extend((f"-L{path.parent}", argument))
        elif object_format == "coff" and (
            argument.lower().endswith(".lib") or argument.startswith("/")
        ):
            flags.append(f"-Wl,{argument}")
        else:
            flags.append(argument)
    return flags


def native_link_flags_from_manifest(
    manifest: Mapping[str, object],
    *,
    object_format: str,
    runtime_lib: Path | None = None,
) -> list[str]:
    try:
        runtime_build_identity = RuntimeBuildIdentity.from_dict(
            manifest.get("runtime_build_identity")
        )
    except (TypeError, ValueError) as exc:
        raise NativeLinkDependencyManifestError(
            f"native link manifest runtime build identity is invalid: {exc}"
        ) from exc
    _require_object_format(runtime_build_identity, object_format)
    try:
        custody, entries = validate_native_link_custody(
            manifest.get("custody"),
            context=os.fspath(runtime_lib) if runtime_lib is not None else "manifest",
        )
    except NativeLinkCustodyError as exc:
        raise NativeLinkDependencyManifestError(str(exc)) from exc
    items = _validated_link_plan(
        manifest.get("link_plan"),
        entries=entries,
        object_format=object_format,
        context=os.fspath(runtime_lib) if runtime_lib is not None else "manifest",
    )
    if entries and runtime_lib is None:
        raise NativeLinkDependencyManifestError(
            "native link manifest requires its adjacent custody archive"
        )
    try:
        custody_paths = (
            ensure_native_link_custody(runtime_lib, custody)
            if runtime_lib is not None
            else {}
        )
    except NativeLinkCustodyError as exc:
        raise NativeLinkDependencyManifestError(str(exc)) from exc
    return _native_link_flags(
        items,
        object_format=object_format,
        custody_paths=custody_paths,
    )


def read_native_link_flags(
    runtime_lib: Path,
    *,
    target_triple: str | None,
    object_format: str,
    runtime_build_identity: RuntimeBuildIdentity,
) -> list[str]:
    _require_object_format(runtime_build_identity, object_format)
    manifest, items = _read_native_link_dependency_manifest(
        runtime_lib,
        target_triple=target_triple,
        runtime_build_identity=runtime_build_identity,
    )
    try:
        custody, _entries = validate_native_link_custody(
            manifest.get("custody"),
            context=str(runtime_lib),
        )
        custody_paths = ensure_native_link_custody(runtime_lib, custody)
    except NativeLinkCustodyError as exc:
        raise NativeLinkDependencyManifestError(str(exc)) from exc
    return _native_link_flags(
        items,
        object_format=object_format,
        custody_paths=custody_paths,
    )
