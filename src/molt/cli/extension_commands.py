"""Source-extension metadata and build command authority."""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import os
import platform
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any, Mapping, Sequence
from molt.cli import source_extensions as _source_extensions
from molt.cli import source_extension_cython as _source_extension_cython
from molt.cli.source_extension_reproducibility import (
    _canonical_extension_manifest_for_wheel,
    _source_extension_deterministic_path_args,
)
from molt.cli import dependency_files as _dependency_files
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_bytes,
    _atomic_write_json,
)
from molt.cli.capability_spec import (
    CapabilityInput,
    _parse_capabilities_spec,
)
from molt.c_api_symbols import is_c_api_external_requirement
from molt.cli.command_runtime import (
    _run_completed_command,
)
from molt.cli.config_resolution import (
    _config_value,
)
from molt.cli.deps import _load_toml, _normalize_name
from molt.cli.extension_manifest import (
    _MOLT_C_API_VERSION_RE,
    _coerce_str_list,
    _default_molt_c_api_version,
    _host_target_triple,
    _infer_module_attr_callable_export_payloads,
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _module_parts,
    _normalize_effects,
    _wheel_token,
    _wheel_version_token,
)
from molt.cli.extension_wheel import _write_extension_wheel
from molt.cli.extension_support import module_attr_support_files
from molt.cli.external_native import (
    _source_recompiled_external_package_root,
)
from molt.file_hashing import _sha256_file
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.llvm_wasi_tools import (
    LlvmToolRole,
    resolve_explicit_tool_command,
    resolve_llvm_wasi_tool_family,
)
from molt.cli.models import (
    BuildProfile,
)
from molt.cli.native_toolchain import _zig_target_query
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import (
    _find_molt_root,
    _find_project_root,
    _require_molt_root,
)
from molt.cli.target_python import (
    _resolve_target_python_version,
)
from molt.cli.setup_readiness import _ensure_rustup_target
from molt.cli.source_extension_toolchain import (
    _materialize_source_extension_target_metadata,
    _normalize_source_extension_abi_tier,
    _normalize_source_extension_metadata_target,
    _resolve_source_extension_wasm_toolchain,
    _source_extension_c_commands,
    _source_extension_include_dirs_for_abi_tier,
    _source_extension_python_header_for_abi_tier,
)
from molt.cli.source_extension_target import (
    resolve_source_extension_target_plan,
    source_extension_artifact_path,
)
from molt.cli.source_extension_link_requirements import (
    source_extension_link_requirements,
)
from molt.cli.wasm_toolchain import (
    normalize_wasi_sysroot,
    resolve_wasi_sysroot,
    wasi_libcxx_include_dir,
)
from molt._wasm_runtime_exports import wasm_static_link_runtime_symbols_for_imports

_SOURCE_EXTENSION_CPP_SUFFIXES = {".cc", ".cpp", ".cxx", ".c++", ".mm"}


def _sysroot_arg_value(args: list[str]) -> str | None:
    index = 0
    while index < len(args):
        part = args[index]
        if part in {"--sysroot", "-isysroot"}:
            if index + 1 < len(args):
                return args[index + 1]
            return ""
        if part.startswith("--sysroot="):
            return part.split("=", 1)[1]
        index += 1
    return None


def _extension_source_text_by_path(source_paths: list[Path]) -> dict[Path, str]:
    return {
        source_path: source_path.read_text(encoding="utf-8", errors="replace")
        for source_path in source_paths
    }


def extension_metadata(
    *,
    target: str | None = None,
    out_dir: str | None = None,
    abi_tier: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    del verbose
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "extension-metadata")
    if root_error is not None:
        return root_error
    assert root is not None
    try:
        target_triple = _normalize_source_extension_metadata_target(target)
        normalized_abi_tier = _normalize_source_extension_abi_tier(abi_tier)
    except ValueError as exc:
        return _fail(str(exc), json_output, command="extension-metadata")

    output_root = Path(out_dir).expanduser() if out_dir else Path("dist")
    if not output_root.is_absolute():
        output_root = (Path.cwd() / output_root).absolute()
    metadata, errors = _materialize_source_extension_target_metadata(
        molt_root=root,
        out_dir=output_root,
        target_triple=target_triple,
        abi_tier=normalized_abi_tier,
    )
    if metadata is None:
        return _fail(
            "Source-extension target metadata errors: " + "; ".join(errors),
            json_output,
            command="extension-metadata",
        )
    data = dict(metadata.payload)
    data["paths"] = dict(data["paths"])
    data["paths"]["out_dir"] = str(metadata.out_dir)
    data["paths"]["pkg_config_dir"] = str(metadata.pkg_config_dir)
    data["paths"]["python_pc"] = str(metadata.python_pc)
    data["paths"]["meson_cross"] = str(metadata.meson_cross)
    data["paths"]["sidecar"] = str(metadata.sidecar)
    data["digest"] = metadata.digest
    if json_output:
        _emit_json(_json_payload("extension-metadata", "ok", data=data), True)
    else:
        print(f"Wrote source-extension target metadata: {metadata.sidecar}")
        print(f"Meson cross file: {metadata.meson_cross}")
        print(f"Python pkg-config: {metadata.python_pc}")
    return 0


def _source_plan_include_paths_for_abi(
    include_paths: Sequence[Path],
    *,
    python_header: Path,
) -> list[Path]:
    selected_python_header = python_header.resolve()
    filtered: list[Path] = []
    for include_path in include_paths:
        candidate_python_header = (include_path / "Python.h").resolve()
        if (
            candidate_python_header.is_file()
            and candidate_python_header != selected_python_header
        ):
            continue
        filtered.append(include_path)
    return filtered


def _source_plan_abi_include_order(
    abi_include_roots: Sequence[Path],
    *,
    python_header: Path,
) -> tuple[Path, list[Path]]:
    python_include_root = python_header.resolve().parent
    fallback_roots: list[Path] = []
    seen = {python_include_root}
    for include_root in abi_include_roots:
        resolved = include_root.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        fallback_roots.append(include_root)
    return python_include_root, fallback_roots


def _source_extension_compile_command_for_source(
    *,
    source_path: Path,
    cc_cmd: Sequence[str],
    cxx_cmd: Sequence[str],
) -> list[str]:
    if source_path.suffix.lower() in _SOURCE_EXTENSION_CPP_SUFFIXES:
        return list(cxx_cmd)
    return list(cc_cmd)


def _extension_export_package(module_parts: list[str]) -> str:
    return module_parts[0]


def _extension_export_config_errors(errors: list[str]) -> list[str]:
    return [
        error.replace("extension_manifest.json", "tool.molt.extension", 1)
        for error in errors
    ]


def _extension_manifest_public_exports(
    extension_meta: Mapping[str, Any],
    *,
    package: str,
    errors: list[str],
) -> tuple[list[str], list[dict[str, Any]]]:
    export_manifest = {
        "python_exports": extension_meta.get("python_exports")
        or extension_meta.get("python-exports"),
        "callable_exports": extension_meta.get("callable_exports")
        or extension_meta.get("callable-exports"),
    }
    export_errors: list[str] = []
    python_exports = _manifest_dotted_name_tuple(
        export_manifest,
        "python_exports",
        package=package,
        errors=export_errors,
    )
    callable_exports = _manifest_callable_exports(
        export_manifest,
        package=package,
        errors=export_errors,
    )
    errors.extend(_extension_export_config_errors(export_errors))
    return (
        list(python_exports),
        [export.digest_payload() for export in callable_exports],
    )


def _source_plan_skipped_generated_sources_warning(
    source_plan: _source_extensions._SourceExtensionBuildPlan | None,
) -> str | None:
    if source_plan is None or not source_plan.skipped_generated_sources:
        return None
    preview = [str(path) for path in source_plan.skipped_generated_sources[:8]]
    remaining = len(source_plan.skipped_generated_sources) - len(preview)
    suffix = f"; +{remaining} more" if remaining > 0 else ""
    noun = "source" if len(source_plan.skipped_generated_sources) == 1 else "sources"
    return (
        "source_plan skipped "
        f"{len(source_plan.skipped_generated_sources)} cleaned generated {noun} "
        "absent from disk: " + ", ".join(preview) + suffix
    )


def extension_build(
    project: str | None = None,
    out_dir: str | None = None,
    module: str | None = None,
    molt_abi: str | None = None,
    capabilities: CapabilityInput | None = None,
    provided_capsules: str | list[str] | None = None,
    python_export: str | list[str] | None = None,
    callable_export_json: str | list[str] | None = None,
    support_file: Any = None,
    deterministic: bool = True,
    profile: BuildProfile = "release",
    python_version: str | None = None,
    target: str | None = None,
    source_plan: str | None = None,
    source_plan_target: str | None = None,
    source_plan_source_root: str | None = None,
    source_plan_build_root: str | None = None,
    source_plan_compile_commands: str | None = None,
    source_plan_exclude_linked_static_libraries: list[str] | None = None,
    source_plan_ninja_command: Sequence[str] | None = None,
    abi_tier: str | None = None,
    tool_commands: Mapping[str, Sequence[str]] | None = None,
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
            command="extension-build",
        )

    pyproject = _load_toml(project_root / "pyproject.toml")
    project_meta = pyproject.get("project")
    extension_meta_raw = _config_value(pyproject, ["tool", "molt", "extension"])
    errors: list[str] = []
    warnings: list[str] = []
    cli_extension_config = any(
        value is not None
        for value in (
            module,
            source_plan,
            source_plan_target,
            source_plan_source_root,
            source_plan_build_root,
            source_plan_compile_commands,
            source_plan_exclude_linked_static_libraries,
            abi_tier,
            provided_capsules,
            python_export,
            callable_export_json,
            support_file,
            python_version,
        )
    )

    if not isinstance(project_meta, dict):
        return _fail(
            "pyproject.toml must contain a [project] table.",
            json_output,
            command="extension-build",
        )
    if not isinstance(extension_meta_raw, dict):
        if cli_extension_config:
            extension_meta: dict[str, Any] = {}
        else:
            return _fail(
                "pyproject.toml must contain [tool.molt.extension], or pass "
                "--module and --source-plan for an upstream build-plan target.",
                json_output,
                command="extension-build",
            )
    else:
        extension_meta = extension_meta_raw

    try:
        extension_target_python = _resolve_target_python_version(
            explicit=python_version,
            build_config=extension_meta,
            project_root=project_root,
        )
    except ValueError as exc:
        return _fail(str(exc), json_output, command="extension-build")

    if source_plan is not None and module is None and "module" not in extension_meta:
        return _fail(
            "--source-plan requires --module when [tool.molt.extension].module "
            "is not configured.",
            json_output,
            command="extension-build",
        )

    cli_python_exports = _coerce_str_list(
        python_export,
        "--python-export",
        errors,
    )
    callable_export_json_items = _coerce_str_list(
        callable_export_json,
        "--callable-export-json",
        errors,
    )
    cli_callable_exports: list[dict[str, Any]] = []
    for index, item in enumerate(callable_export_json_items):
        try:
            payload = json.loads(item)
        except json.JSONDecodeError as exc:
            errors.append(f"--callable-export-json[{index}] must be JSON: {exc}")
            continue
        if not isinstance(payload, dict):
            errors.append(f"--callable-export-json[{index}] must be a JSON object")
            continue
        cli_callable_exports.append(payload)
    extension_export_meta: dict[str, Any] = dict(extension_meta)
    if cli_python_exports:
        extension_export_meta["python_exports"] = cli_python_exports
    if cli_callable_exports:
        extension_export_meta["callable_exports"] = cli_callable_exports
    raw_support_files: list[Any] = []
    extension_support_files = extension_meta.get("support_files") or extension_meta.get(
        "support-files"
    )
    if extension_support_files is not None:
        if isinstance(extension_support_files, list):
            raw_support_files.extend(extension_support_files)
        else:
            errors.append("tool.molt.extension.support_files must be a list")
    if support_file is not None:
        if isinstance(support_file, str):
            raw_support_files.append(support_file)
        elif isinstance(support_file, list):
            raw_support_files.extend(support_file)
        else:
            errors.append("--support-file must be a string or list")

    project_name = project_meta.get("name")
    project_version = project_meta.get("version")
    if not isinstance(project_name, str) or not project_name.strip():
        errors.append("project.name must be a non-empty string")
    if not isinstance(project_version, str) or not project_version.strip():
        errors.append("project.version must be a non-empty string")

    module_name = module or extension_meta.get("module")
    if not isinstance(module_name, str):
        errors.append("tool.molt.extension.module or --module must be a string")
        module_name = ""
    module_parts = _module_parts(module_name)
    if module_parts is None:
        errors.append("tool.molt.extension.module must be a dotted Python identifier")
        module_parts = ["extension"]
    source_paths: list[Path] = []
    include_paths: list[Path] = []
    compile_args: list[str] = []
    link_args: list[str] = []
    loaded_source_plan: _source_extensions._SourceExtensionBuildPlan | None = None
    source_plan_config_raw = extension_meta.get("source_plan") or extension_meta.get(
        "source-plan"
    )
    source_plan_config: dict[str, Any] | None = None
    if source_plan_config_raw is not None:
        if not isinstance(source_plan_config_raw, dict):
            errors.append("tool.molt.extension.source_plan must be a table")
        else:
            source_plan_config = dict(source_plan_config_raw)
    if source_plan is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["path"] = source_plan
    if source_plan_target is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["target"] = source_plan_target
    if source_plan_source_root is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["source_root"] = source_plan_source_root
    if source_plan_build_root is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["build_root"] = source_plan_build_root
    if source_plan_compile_commands is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["compile_commands"] = source_plan_compile_commands
    if source_plan_exclude_linked_static_libraries:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["exclude_linked_static_libraries"] = list(
            source_plan_exclude_linked_static_libraries
        )

    if source_plan_config is None:
        raw_sources = _coerce_str_list(
            extension_meta.get("sources"),
            "tool.molt.extension.sources",
            errors,
            allow_empty=False,
        )
        if not raw_sources:
            errors.append(
                "tool.molt.extension.sources must include at least one source"
            )
        for entry in raw_sources:
            source_path = Path(entry).expanduser()
            if not source_path.is_absolute():
                source_path = (project_root / source_path).absolute()
            if not source_path.exists() or not source_path.is_file():
                errors.append(f"source file not found: {source_path}")
                continue
            source_paths.append(source_path)

        include_dirs_raw = _coerce_str_list(
            extension_meta.get("include_dirs") or extension_meta.get("include-dirs"),
            "tool.molt.extension.include_dirs",
            errors,
        )
        for entry in include_dirs_raw:
            include_path = Path(entry).expanduser()
            if not include_path.is_absolute():
                include_path = (project_root / include_path).absolute()
            include_paths.append(include_path)

        compile_args = _coerce_str_list(
            extension_meta.get("extra_compile_args")
            or extension_meta.get("extra-compile-args"),
            "tool.molt.extension.extra_compile_args",
            errors,
        )
        link_args = _coerce_str_list(
            extension_meta.get("extra_link_args")
            or extension_meta.get("extra-link-args"),
            "tool.molt.extension.extra_link_args",
            errors,
        )
    else:
        manual_authority_fields = [
            field
            for field in (
                "sources",
                "include_dirs",
                "include-dirs",
                "extra_compile_args",
                "extra-compile-args",
                "extra_link_args",
                "extra-link-args",
            )
            if field in extension_meta
        ]
        if manual_authority_fields:
            errors.append(
                "tool.molt.extension.source_plan plus compile_commands.json is "
                "the source/arg authority; remove parallel manual fields: "
                + ", ".join(sorted(manual_authority_fields))
            )
        if not errors:
            loaded_source_plan, source_plan_errors = (
                _source_extensions._load_source_extension_build_plan(
                    project_root=project_root,
                    module_name=module_name,
                    plan_config=source_plan_config,
                )
            )
            errors.extend(source_plan_errors)
        if loaded_source_plan is not None:
            source_paths = [
                unit.source_path for unit in loaded_source_plan.compile_units
            ]
            include_paths = list(loaded_source_plan.include_dirs)
            compile_args = list(loaded_source_plan.compile_args)
            link_args = list(loaded_source_plan.link_args)

    provided_capsules_input: str | list[str] | None = provided_capsules
    if provided_capsules_input is None:
        configured_provided_capsules = extension_meta.get(
            "provided_capsules"
        ) or extension_meta.get("provided-capsules")
    else:
        configured_provided_capsules = provided_capsules_input
    provided_capsules_tuple = tuple(
        sorted(
            set(
                _coerce_str_list(
                    configured_provided_capsules,
                    "tool.molt.extension.provided_capsules",
                    errors,
                )
            )
        )
    )
    try:
        normalized_abi_tier = _normalize_source_extension_abi_tier(
            abi_tier or extension_meta.get("abi_tier") or extension_meta.get("abi-tier")
        )
    except ValueError as exc:
        errors.append(str(exc))
        normalized_abi_tier = "source-compat"

    effects = _normalize_effects(extension_meta.get("effects"))
    determinism_mode = "deterministic" if deterministic else "nondet"
    determinism_raw = extension_meta.get("determinism")
    if determinism_raw is not None:
        if not isinstance(determinism_raw, str):
            errors.append(
                "tool.molt.extension.determinism must be 'deterministic' or 'nondet'"
            )
        else:
            normalized = determinism_raw.strip().lower()
            if normalized not in {"deterministic", "nondet"}:
                errors.append(
                    "tool.molt.extension.determinism must be 'deterministic' or "
                    "'nondet'"
                )
            else:
                determinism_mode = normalized
    if deterministic:
        determinism_mode = "deterministic"

    try:
        target_plan = resolve_source_extension_target_plan(
            target,
            host_target_triple=_host_target_triple(),
            host_platform=sys.platform,
            host_arch=platform.machine(),
        )
    except ValueError as exc:
        errors.append(str(exc))
        target_plan = resolve_source_extension_target_plan(
            "native",
            host_target_triple=_host_target_triple(),
            host_platform=sys.platform,
            host_arch=platform.machine(),
        )
    runtime_target_triple = target_plan.compiler_target_triple
    wasm_static_link = target_plan.is_wasm
    if loaded_source_plan is not None:
        errors.extend(
            _source_extensions._validate_source_extension_build_plan_target(
                loaded_source_plan,
                target_triple=runtime_target_triple,
            )
        )

    capability_input: CapabilityInput | None = capabilities
    if capability_input is None:
        cfg_capabilities = extension_meta.get("capabilities")
        if isinstance(cfg_capabilities, (str, list, dict)):
            capability_input = cfg_capabilities
    if capability_input is None:
        errors.append(
            "Missing extension capabilities: set tool.molt.extension.capabilities "
            "or pass --capabilities."
        )
    capabilities_list: list[str] = []
    capability_profiles: list[str] = []
    if capability_input is not None:
        spec = _parse_capabilities_spec(capability_input)
        if spec.errors:
            errors.append("Invalid capabilities: " + ", ".join(spec.errors))
        else:
            capabilities_list = spec.capabilities or []
            capability_profiles = spec.profiles
    python_exports, callable_exports = _extension_manifest_public_exports(
        extension_export_meta,
        package=_extension_export_package(module_parts),
        errors=errors,
    )
    support_files = module_attr_support_files(
        raw_support_files,
        field_name="tool.molt.extension.support_files",
        source_root=project_root,
        package=_extension_export_package(module_parts),
        extension_module=".".join(module_parts),
        callable_exports=callable_exports,
        target_python=extension_target_python,
        errors=errors,
    )
    source_recompiled_root = _source_recompiled_external_package_root(
        ".".join(module_parts)
    )
    if source_recompiled_root and not (python_exports or callable_exports):
        errors.append(
            "Source-recompiled extension builds for "
            f"{source_recompiled_root!r} must declare tool.molt.extension."
            "python_exports or tool.molt.extension.callable_exports; native "
            "artifact reachability is manifest-symbol custody, not package "
            "directory ancestry"
        )

    cwd_root = _find_project_root(Path.cwd())
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "extension-build")
    if root_error is not None:
        return root_error

    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        False,
        "extension-build",
    )
    if lock_error is not None:
        return lock_error

    default_abi = _default_molt_c_api_version(molt_root)
    abi_raw = molt_abi or extension_meta.get("molt_c_api_version") or default_abi
    if not isinstance(abi_raw, str):
        errors.append("molt ABI must be a string")
        abi_raw = default_abi
    abi_version = abi_raw.strip()
    if _MOLT_C_API_VERSION_RE.match(abi_version) is None:
        errors.append(
            "Invalid molt ABI version. Expected MAJOR[.MINOR[.PATCH]] "
            f"(got {abi_version!r})."
        )
    abi_major = abi_version.split(".", 1)[0] if abi_version else "0"
    abi_tag = f"molt_abi{abi_major}"

    if errors:
        return _fail(
            "Extension build configuration errors: " + "; ".join(errors),
            json_output,
            command="extension-build",
        )

    source_c_api_requirements: (
        _source_extensions._SourceExtensionCAPIRequirements | None
    ) = None
    if loaded_source_plan is None:
        try:
            source_text_by_path = _extension_source_text_by_path(source_paths)
            inferred_callable_exports = _infer_module_attr_callable_export_payloads(
                source_text_by_path.values(),
                python_exports=python_exports,
                explicit_callable_exports=callable_exports,
                effects=effects,
                deterministic=determinism_mode == "deterministic",
            )
            if inferred_callable_exports:
                callable_exports = [
                    *callable_exports,
                    *[dict(export) for export in inferred_callable_exports],
                ]
                callable_exports = sorted(
                    callable_exports,
                    key=lambda export: (
                        str(export.get("module")),
                        str(export.get("name")),
                    ),
                )
        except OSError as exc:
            return _fail(
                f"Failed scanning extension C/API source surface: {exc}",
                json_output,
                command="extension-build",
            )

    output_root = Path(out_dir).expanduser() if out_dir else Path("dist")
    if not output_root.is_absolute():
        output_root = (project_root / output_root).absolute()
    output_root.mkdir(parents=True, exist_ok=True)

    if runtime_target_triple:
        _ensure_rustup_target(runtime_target_triple, warnings)

    abi_include_roots = _source_extension_include_dirs_for_abi_tier(
        molt_root=molt_root,
        abi_tier=normalized_abi_tier,
    )
    missing_abi_include_roots = [
        include_root for include_root in abi_include_roots if not include_root.exists()
    ]
    if missing_abi_include_roots:
        return _fail(
            "Missing Molt ABI header roots: "
            + ", ".join(str(path) for path in missing_abi_include_roots),
            json_output,
            command="extension-build",
        )
    python_header = _source_extension_python_header_for_abi_tier(
        molt_root=molt_root,
        abi_tier=normalized_abi_tier,
    )
    if loaded_source_plan is None:
        source_c_api_requirements, capi_error = (
            _source_extensions._source_extension_required_c_api_by_source(
                molt_root=molt_root,
                source_paths=source_paths,
                python_header=python_header,
                definition_header_roots=[
                    *include_paths,
                    project_root,
                    *(source_path.parent for source_path in source_paths),
                ],
                compile_args_by_source={
                    source_path: compile_args for source_path in source_paths
                },
                preprocessor_defined_symbols=list(target_plan.preprocessor_symbols),
            )
        )
        if capi_error is not None:
            return _fail(capi_error, json_output, command="extension-build")
        assert source_c_api_requirements is not None

    effective_tool_commands: Mapping[str, Sequence[str]] = tool_commands or {}
    cc_cmd: list[str] = []
    wasi_sysroot: Path | None = None
    if wasm_static_link:
        target_arg = runtime_target_triple or "wasm32-wasip1"
        if not effective_tool_commands:
            wasm_toolchain = _resolve_source_extension_wasm_toolchain()
            if not wasm_toolchain.ok:
                return _fail(
                    "WASM source-extension build requires a valid wasm compiler "
                    "and linker toolchain: " + wasm_toolchain.detail,
                    json_output,
                    command="extension-build",
                )
            effective_tool_commands = _source_extension_c_commands(
                toolchain=wasm_toolchain,
                target_triple=target_arg,
            )
            wasi_sysroot = wasm_toolchain.wasi_sysroot
        cc_cmd = list(effective_tool_commands.get("c", ()))
        if not cc_cmd:
            return _fail(
                "Canonical LLVM/WASI tool authority has no C compiler command.",
                json_output,
                command="extension-build",
            )
        explicit_sysroot = _sysroot_arg_value([*cc_cmd, *compile_args])
        if explicit_sysroot is not None:
            wasi_sysroot = normalize_wasi_sysroot(explicit_sysroot)
        if wasi_sysroot is None:
            wasi_sysroot = resolve_wasi_sysroot()
        if wasi_sysroot is None:
            return _fail(
                "WASM extension build requires a WASI sysroot containing "
                "include/errno.h. Set MOLT_WASI_SYSROOT, WASI_SYSROOT, or "
                "WASI_SDK_PATH.",
                json_output,
                command="extension-build",
            )
    elif runtime_target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = runtime_target_triple
        if cross_cc:
            try:
                cc_cmd = list(
                    resolve_explicit_tool_command(
                        cross_cc,
                        label="MOLT_CROSS_CC",
                    )
                )
            except ValueError as exc:
                return _fail(
                    str(exc),
                    json_output,
                    command="extension-build",
                )
        elif shutil.which("zig"):
            try:
                cc_cmd = [
                    *resolve_explicit_tool_command("zig", label="zig"),
                    "cc",
                ]
            except ValueError as exc:
                return _fail(
                    str(exc),
                    json_output,
                    command="extension-build",
                )
            normalized = _zig_target_query(runtime_target_triple)
            if normalized != runtime_target_triple:
                warnings.append(
                    f"Zig target normalized to {normalized} from {runtime_target_triple}."
                )
            target_arg = normalized
        else:
            return _fail(
                "Cross-target extension build requires zig or MOLT_CROSS_CC "
                f"(missing for {runtime_target_triple}).",
                json_output,
                command="extension-build",
            )
        if not cc_cmd:
            return _fail(
                "Compiler command is empty. Set MOLT_CROSS_CC or install zig.",
                json_output,
                command="extension-build",
            )
        cc_cmd.extend(["-target", target_arg])
    elif not effective_tool_commands:
        try:
            cc_cmd = list(
                resolve_explicit_tool_command(
                    os.environ.get("CC", "clang"),
                    label="CC",
                )
            )
        except ValueError as exc:
            return _fail(
                str(exc),
                json_output,
                command="extension-build",
            )

    if not wasm_static_link:
        try:
            if effective_tool_commands:
                native_commands = {
                    role: tuple(command)
                    for role, command in effective_tool_commands.items()
                }
            else:
                explicit_native_tools: dict[LlvmToolRole, tuple[str, ...]] = {
                    "cc": tuple(cc_cmd)
                }
                configured_cxx = os.environ.get(
                    "MOLT_CROSS_CXX" if runtime_target_triple else "CXX",
                    "",
                ).strip()
                if configured_cxx:
                    explicit_native_tools["cxx"] = resolve_explicit_tool_command(
                        configured_cxx,
                        label="MOLT_CROSS_CXX" if runtime_target_triple else "CXX",
                    )
                elif Path(cc_cmd[0]).name.lower() in {"zig", "zig.exe"}:
                    explicit_native_tools["cxx"] = (
                        cc_cmd[0],
                        "c++",
                        *cc_cmd[2:],
                    )
                native_family = resolve_llvm_wasi_tool_family(
                    explicit_commands=explicit_native_tools,
                    sibling_directories=(Path(cc_cmd[0]).parent,),
                )
                native_commands = {
                    role: tool.command
                    for role, tool in {
                        "c": native_family.cc,
                        "cpp": native_family.cxx,
                        "ar": native_family.ar,
                        "nm": native_family.nm,
                        "ranlib": native_family.ranlib,
                    }.items()
                    if tool is not None
                }
                # Explicit compiler selection is target policy.  Family
                # discovery supplies sibling archive and symbol tools; it must
                # never replace the compiler command or its target arguments.
                native_commands["c"] = explicit_native_tools["cc"]
                if "cxx" in explicit_native_tools:
                    native_commands["cpp"] = explicit_native_tools["cxx"]
        except (OSError, ValueError) as exc:
            return _fail(
                f"Native source-extension tool family is invalid: {exc}",
                json_output,
                command="extension-build",
            )
        required_native_roles = {"c", "ar", "nm"}
        if any(
            path.suffix.lower() in _SOURCE_EXTENSION_CPP_SUFFIXES
            for path in source_paths
        ):
            required_native_roles.add("cpp")
        missing_native_roles = sorted(required_native_roles - native_commands.keys())
        if missing_native_roles:
            return _fail(
                "Native static source-extension builds require one canonical LLVM "
                "compiler/archive/symbol tool family; missing: "
                + ", ".join(missing_native_roles),
                json_output,
                command="extension-build",
            )
        effective_tool_commands = native_commands
        cc_cmd = list(native_commands["c"])

    dist_name = _normalize_name(str(project_name)).replace("-", "_")
    wheel_version = _wheel_version_token(str(project_version))
    target_triple = target_plan.target_triple
    platform_tag = _wheel_token(target_triple)
    python_tag = "py3"
    wheel_name = (
        f"{dist_name}-{wheel_version}-{python_tag}-{abi_tag}-{platform_tag}.whl"
    )
    wheel_path = output_root / wheel_name

    build_env = os.environ.copy()
    # Reproducibility is a build input, never ambient policy.
    if deterministic or profile == "release":
        build_env["SOURCE_DATE_EPOCH"] = "315532800"

    module_rel = source_extension_artifact_path(module_parts, target_plan)
    init_symbol = f"PyInit_{module_parts[-1]}"
    compile_commands: list[list[str]] = []
    link_command: list[str] = []
    wasi_sysroot_path = str(wasi_sysroot) if wasm_static_link else None

    with tempfile.TemporaryDirectory(prefix="molt_ext_build_", dir=output_root) as td:
        build_tmp = Path(td)
        object_paths: list[Path] = []
        object_facts: list[_source_extensions._SourceExtensionObjectFact] = []
        # R73.2: a Cython extension's shipped C may have been emitted with
        # ``--shared scipy._cyutility`` (an unsatisfiable shared-utility import).
        # Molt owns how it gets a Cython extension's C: regenerate it STANDALONE
        # from the package's own ``.pyx`` so the module embeds its utilities and
        # imports no shared-utility module. One custody path, no host fallback.
        cython_regenerations: dict[Path, _source_extension_cython.CythonRegeneration]
        cython_regenerations = {}
        if loaded_source_plan is not None:
            pyx_candidates = [
                path
                for path in (
                    *loaded_source_plan.non_compiled_inputs,
                    *loaded_source_plan.sources,
                    *loaded_source_plan.generated_sources,
                )
                if path.suffix.lower() == ".pyx"
            ]
            cython_targets: list[tuple[Path, Path]] = []
            for unit in loaded_source_plan.compile_units:
                pyx_path = _source_extension_cython.pair_generated_c_with_pyx(
                    generated_c=unit.source_path,
                    pyx_candidates=pyx_candidates,
                )
                if pyx_path is not None and pyx_path.is_file():
                    cython_targets.append((unit.source_path.resolve(), pyx_path))
            if cython_targets:
                cython_requirement = (
                    _source_extension_cython.cython_build_requirement_from_pyproject(
                        pyproject
                    )
                )
                cython_python_exe = sys.executable
                cython_version, provision_error = (
                    _source_extension_cython.provision_cython(
                        python_exe=cython_python_exe,
                        requirement=cython_requirement,
                    )
                )
                if provision_error is not None:
                    return _fail(
                        provision_error,
                        json_output,
                        command="extension-build",
                    )
                assert cython_version is not None
                # Keep generated C under the source plan's build root until the
                # package producer content-addresses every compiled input and
                # rewrites the final sidecar to its sealed relative path.
                cython_out_dir = (
                    loaded_source_plan.build_root / "molt_cython_standalone"
                )
                for original_c, pyx_path in cython_targets:
                    plan_include_dirs = [
                        unit.include_dirs
                        for unit in loaded_source_plan.compile_units
                        if unit.source_path.resolve() == original_c
                    ]
                    flat_includes = [
                        include_dir
                        for includes in plan_include_dirs
                        for include_dir in includes
                    ]
                    regeneration, regen_error = (
                        _source_extension_cython.regenerate_cython_c_standalone(
                            pyx_path=pyx_path,
                            original_c=original_c,
                            out_dir=cython_out_dir,
                            include_dirs=flat_includes,
                            cython_version=cython_version,
                            python_exe=cython_python_exe,
                            package_roots=(
                                loaded_source_plan.source_root,
                                loaded_source_plan.build_root,
                            ),
                            ninja_command=tuple(source_plan_ninja_command or ()),
                        )
                    )
                    if regen_error is not None:
                        return _fail(
                            regen_error,
                            json_output,
                            command="extension-build",
                        )
                    assert regeneration is not None
                    if normalized_abi_tier != "cpython-abi":
                        return _fail(
                            "Standalone Cython extensions require --abi-tier "
                            "cpython-abi; the legacy source-compat/limited-API "
                            "Cython lane has been removed",
                            json_output,
                            command="extension-build",
                        )
                    cython_regenerations[original_c] = regeneration
        for idx, source_path in enumerate(source_paths):
            regeneration = cython_regenerations.get(source_path.resolve())
            if regeneration is not None:
                source_path = regeneration.regenerated_c
            plan_unit = (
                loaded_source_plan.compile_units[idx]
                if loaded_source_plan is not None
                else None
            )
            unit_include_paths = (
                list(plan_unit.include_dirs) if plan_unit is not None else include_paths
            )
            if regeneration is not None:
                unit_include_paths = [
                    *regeneration.cimport_header_include_dirs,
                    *unit_include_paths,
                ]
            if plan_unit is not None:
                unit_include_paths = _source_plan_include_paths_for_abi(
                    unit_include_paths,
                    python_header=python_header,
                )
            unit_compile_args = (
                list(plan_unit.compile_args) if plan_unit is not None else compile_args
            )
            unit_cc_cmd = _source_extension_compile_command_for_source(
                source_path=source_path,
                cc_cmd=cc_cmd,
                cxx_cmd=effective_tool_commands.get("cpp", ()),
            )
            if not unit_cc_cmd:
                return _fail(
                    "Canonical source-extension tool authority has no compiler "
                    f"for {source_path.suffix or 'this source kind'}.",
                    json_output,
                    command="extension-build",
                )
            object_path = build_tmp / f"{idx}_{source_path.stem}.o"
            cmd = [*unit_cc_cmd, "-c", str(source_path), "-o", str(object_path)]
            dependency_file: Path | None = None
            if loaded_source_plan is not None:
                driver = Path(unit_cc_cmd[0]).name.lower() if unit_cc_cmd else ""
                if not any(name in driver for name in ("clang", "gcc", "zig")):
                    return _fail(
                        "Source-plan compilation requires a compiler that emits "
                        f"canonical Make depfiles; unsupported driver: {driver}",
                        json_output,
                        command="extension-build",
                    )
                dependency_file = build_tmp / f"{idx}_{source_path.stem}.d"
                cmd.extend(
                    [
                        "-MD",
                        "-MF",
                        str(dependency_file),
                        "-MT",
                        object_path.name,
                    ]
                )
            cmd.extend(f"-D{symbol}=1" for symbol in target_plan.preprocessor_symbols)
            if plan_unit is not None:
                python_include_root, fallback_abi_include_roots = (
                    _source_plan_abi_include_order(
                        abi_include_roots,
                        python_header=python_header,
                    )
                )
                cmd.extend(["-I", str(python_include_root)])
                cmd.extend(["-I", str(project_root)])
                for include_path in unit_include_paths:
                    cmd.extend(["-I", str(include_path)])
                for include_path in fallback_abi_include_roots:
                    cmd.extend(["-I", str(include_path)])
            else:
                for include_path in abi_include_roots:
                    cmd.extend(["-I", str(include_path)])
                cmd.extend(["-I", str(project_root)])
                for include_path in unit_include_paths:
                    cmd.extend(["-I", str(include_path)])
            if target_plan.requires_position_independent_code:
                cmd.append("-fPIC")
            if deterministic:
                cmd.extend(
                    _source_extension_deterministic_path_args(
                        compiler_command=unit_cc_cmd,
                        roots=(
                            (project_root, ".molt/source"),
                            (
                                loaded_source_plan.build_root
                                if loaded_source_plan is not None
                                else None,
                                ".molt/build",
                            ),
                            (output_root, ".molt/output"),
                            (build_tmp, ".molt/objects"),
                            (molt_root, ".molt/repo"),
                            (wasi_sysroot, ".molt/wasi-sysroot"),
                            (Path(sys.prefix), ".molt/python"),
                            (
                                Path(unit_cc_cmd[0]).resolve().parent,
                                ".molt/toolchain",
                            ),
                        ),
                    )
                )
            if plan_unit is not None:
                cmd.extend(
                    _source_extensions._source_extension_gc_compile_args(
                        target_triple=runtime_target_triple,
                    )
                )
                cmd.extend(
                    _source_extensions._source_extension_wasm_compile_args(
                        target_triple=runtime_target_triple,
                        cc_cmd=unit_cc_cmd,
                    )
                )
                # C++ sources on wasm need the sysroot's libc++ include tree.
                # WASI multilib sysroots hide libc++ under
                # include/<target>/{eh,noeh}/c++/v1 (flat include/c++/v1 empty),
                # so clang++ can't auto-find <atomic>/<vector>. numpy's C++
                # sources (einsum.cpp, npysort/*.cpp, ...) fail
                # `fatal error: 'atomic' file not found` without it. molt enables
                # exception handling (-mexception-handling), so select the eh
                # variant.
                if wasm_static_link and source_path.suffix.lower() in {
                    ".cpp",
                    ".cxx",
                    ".cc",
                }:
                    libcxx_inc = wasi_libcxx_include_dir(
                        wasi_sysroot,
                        target_triple=runtime_target_triple,
                        exceptions=True,
                    )
                    if libcxx_inc is not None:
                        cmd.extend(["-I", str(libcxx_inc)])
            # Regenerated Cython C has one explicit full-CPython ABI profile.
            # The source-compat/limited-API branch was deleted above; only a
            # successful standalone regeneration on cpython-abi receives these
            # selectors, immediately before the source-plan unit authority.
            if regeneration is not None:
                cmd.extend(_source_extension_cython.CYTHON_CPYTHON_ABI_COMPILE_ARGS)
            cmd.extend(
                _source_extensions._source_extension_replay_compile_args(
                    unit_compile_args,
                )
            )
            result = _run_completed_command(
                cmd,
                cwd=project_root,
                env=build_env,
                capture_output=True,
                memory_guard_prefix="MOLT_BUILD",
            )
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip()
                if not detail:
                    detail = f"compiler exited with code {result.returncode}"
                return _fail(
                    f"Failed compiling {source_path.name}: {detail}",
                    json_output,
                    command="extension-build",
                )
            compile_commands.append(cmd)
            object_paths.append(object_path)
            if loaded_source_plan is not None:
                assert dependency_file is not None
                dependency_paths, dependency_error = (
                    _dependency_files.parse_make_depfile(
                        dependency_file,
                        cwd=project_root,
                        producer="compiler",
                    )
                )
                if dependency_error is not None:
                    return _fail(
                        dependency_error,
                        json_output,
                        command="extension-build",
                    )
                assert dependency_paths is not None
                object_fact, object_fact_error = (
                    _source_extensions._source_extension_object_fact(
                        source_path=source_path,
                        object_path=object_path,
                        compile_command=cmd,
                        dependency_paths=(
                            *dependency_paths,
                            *(
                                tuple(
                                    dependency.path
                                    for dependency in regeneration.dependencies
                                )
                                if regeneration is not None
                                else ()
                            ),
                        ),
                        nm_command=effective_tool_commands.get("nm"),
                    )
                )
                if object_fact_error is not None:
                    return _fail(
                        object_fact_error,
                        json_output,
                        command="extension-build",
                    )
                assert object_fact is not None
                object_facts.append(object_fact)
            else:
                object_fact, object_fact_error = (
                    _source_extensions._source_extension_object_fact(
                        source_path=source_path,
                        object_path=object_path,
                        compile_command=cmd,
                        dependency_paths=(source_path,),
                        nm_command=effective_tool_commands.get("nm"),
                    )
                )
                if object_fact_error is not None:
                    return _fail(
                        object_fact_error,
                        json_output,
                        command="extension-build",
                    )
                assert object_fact is not None
                object_facts.append(object_fact)

        source_plan_object_closure, object_closure_errors = (
            _source_extensions._compute_source_extension_object_closure(
                init_symbol=init_symbol,
                object_facts=object_facts,
            )
        )
        if object_closure_errors:
            return _fail(
                "Source extension object closure errors: "
                + "; ".join(object_closure_errors),
                json_output,
                command="extension-build",
            )
        assert source_plan_object_closure is not None
        object_paths = [fact.object_path for fact in source_plan_object_closure.objects]
        if loaded_source_plan is not None:
            (
                source_c_api_requirements,
                capi_error,
            ) = _source_extensions._source_extension_required_c_api_by_source(
                molt_root=molt_root,
                source_paths=[
                    fact.source_path for fact in source_plan_object_closure.objects
                ],
                python_header=python_header,
                definition_header_roots=[
                    *loaded_source_plan.include_dirs,
                    *(
                        fact.source_path.parent
                        for fact in source_plan_object_closure.objects
                    ),
                ],
                compile_args_by_source={
                    unit.source_path: unit.compile_args
                    for unit in loaded_source_plan.compile_units
                },
                preprocessor_defined_symbols=list(target_plan.preprocessor_symbols),
            )
            if capi_error is not None:
                return _fail(capi_error, json_output, command="extension-build")
        assert source_c_api_requirements is not None
        source_c_api_requirements = source_c_api_requirements.restrict_to_link_closure(
            source_plan_object_closure.undefined_symbols
        )
        missing_c_api = list(source_c_api_requirements.missing_symbols)
        fail_fast_c_api = list(source_c_api_requirements.fail_fast_symbols)
        if missing_c_api or fail_fast_c_api:
            details: list[str] = []
            if missing_c_api:
                details.append("missing: " + ", ".join(missing_c_api[:16]))
            if fail_fast_c_api:
                details.append("fail-fast: " + ", ".join(fail_fast_c_api[:16]))
            return _fail(
                "Reachable source extension C/API symbols are unsupported ("
                + "; ".join(details)
                + ")",
                json_output,
                command="extension-build",
            )

        try:
            link_requirements = source_extension_link_requirements(
                link_args,
                target_triple=target_plan.target_triple,
                folded_static_archives=(
                    loaded_source_plan.folded_static_archives
                    if loaded_source_plan is not None
                    else ()
                ),
                path_roots=(
                    *(
                        (
                            loaded_source_plan.build_root,
                            loaded_source_plan.source_root,
                        )
                        if loaded_source_plan is not None
                        else ()
                    ),
                    project_root,
                ),
                publish_root=output_root / module_parts[0],
            )
        except ValueError as exc:
            return _fail(str(exc), json_output, command="extension-build")

        built_extension = build_tmp / module_rel
        built_extension.parent.mkdir(parents=True, exist_ok=True)
        if wasm_static_link:
            if len(object_paths) == 1:
                _atomic_copy_file(object_paths[0], built_extension)
            else:
                wasm_ld_cmd = list((effective_tool_commands or {}).get("ld", ()))
                if not wasm_ld_cmd:
                    return _fail(
                        "Source-extension linker authority is missing the canonical "
                        "LLVM/WASI 'ld' command.",
                        json_output,
                        command="extension-build",
                    )
                link_command = [
                    *wasm_ld_cmd,
                    "-r",
                    *(
                        ["--allow-undefined", "--no-entry"]
                        if loaded_source_plan is not None
                        else []
                    ),
                    *[str(path) for path in object_paths],
                    "-o",
                    str(built_extension),
                ]
                link_result = _run_completed_command(
                    link_command,
                    cwd=project_root,
                    env=build_env,
                    capture_output=True,
                    memory_guard_prefix="MOLT_BUILD",
                )
                if link_result.returncode != 0:
                    detail = link_result.stderr.strip() or link_result.stdout.strip()
                    if not detail:
                        detail = (
                            f"wasm linker exited with code {link_result.returncode}"
                        )
                    return _fail(
                        f"Failed linking wasm relocatable extension object: {detail}",
                        json_output,
                        command="extension-build",
                    )
        else:
            archive_command = list(effective_tool_commands.get("ar", ()))
            if not archive_command:
                return _fail(
                    "Native source-extension archive authority is missing llvm-ar.",
                    json_output,
                    command="extension-build",
                )
            link_command = [
                *archive_command,
                "rcsD",
                str(built_extension),
                *[str(path) for path in object_paths],
            ]
            link_result = _run_completed_command(
                link_command,
                cwd=project_root,
                env=build_env,
                capture_output=True,
                memory_guard_prefix="MOLT_BUILD",
            )
            if link_result.returncode != 0:
                detail = link_result.stderr.strip() or link_result.stdout.strip()
                if not detail:
                    detail = f"archiver exited with code {link_result.returncode}"
                return _fail(
                    f"Failed creating static extension archive: {detail}",
                    json_output,
                    command="extension-build",
                )

        if not built_extension.exists():
            return _fail(
                "Link succeeded but extension artifact is missing.",
                json_output,
                command="extension-build",
            )
        defined_symbols = sorted(
            {
                symbol
                for fact in source_plan_object_closure.objects
                for symbol in fact.defined_symbols
            }
        )
        direct_symbols = sorted(
            {
                str(export.get("symbol"))
                for export in callable_exports
                if export.get("binding") == "direct_symbol"
                and isinstance(export.get("symbol"), str)
                and str(export.get("symbol")).strip()
            }
        )
        missing_direct_symbols = [
            symbol for symbol in direct_symbols if symbol not in defined_symbols
        ]
        if missing_direct_symbols:
            return _fail(
                "Extension direct_symbol callable export(s) missing from the static "
                f"object closure: {', '.join(missing_direct_symbols)}",
                json_output,
                command="extension-build",
            )
        if wasm_static_link:
            _atomic_copy_file(built_extension, output_root / module_rel)

        extension_bytes = built_extension.read_bytes()
        extension_archive_path = module_rel.as_posix()
        runtime_linkage = "static_link"
        artifact_kind = target_plan.artifact_kind
        build_payload: dict[str, Any] = {
            "compiler": cc_cmd,
            "tool_commands": {
                role: list(command)
                for role, command in sorted(effective_tool_commands.items())
            },
            "compiler_target": runtime_target_triple or "native",
            "wasi_sysroot": wasi_sysroot_path,
            "runtime_linkage": runtime_linkage,
            "artifact_kind": artifact_kind,
            "include_dirs": [str(path) for path in abi_include_roots]
            + [str(project_root)]
            + [str(path) for path in include_paths],
            "python_header": str(python_header),
            "extra_compile_args": compile_args,
            "extra_link_args": list(link_requirements.arguments),
            "source_date_epoch": build_env.get("SOURCE_DATE_EPOCH"),
        }
        manifest_payload: dict[str, Any] = {
            "schema_version": 1,
            "name": str(project_name),
            "version": str(project_version),
            "module": ".".join(module_parts),
            "sources": [str(path) for path in source_paths],
            "molt_c_api_version": abi_version,
            "abi_tag": abi_tag,
            "abi_tier": normalized_abi_tier,
            "python_tag": python_tag,
            "target_python": extension_target_python.tag,
            "target_triple": target_triple,
            "platform_tag": platform_tag,
            "loader_kind": "libmolt_source",
            "init_symbol": init_symbol,
            "runtime_linkage": runtime_linkage,
            "artifact_kind": artifact_kind,
            "link_requirements": link_requirements.manifest_payload(),
            "provided_capsules": list(provided_capsules_tuple),
            "capabilities": capabilities_list,
            "capability_profiles": capability_profiles,
            "deterministic": deterministic,
            "determinism": determinism_mode,
            "effects": effects,
            "wheel": wheel_name,
            "extension": extension_archive_path,
            "support_files": [entry.digest_payload() for entry in support_files],
            "build": build_payload,
        }
        if source_c_api_requirements is not None:
            build_payload["source_c_api_scan"] = (
                source_c_api_requirements.manifest_payload()
            )
        source_plan_skipped_sources_warning = (
            _source_plan_skipped_generated_sources_warning(loaded_source_plan)
        )
        if source_plan_skipped_sources_warning is not None:
            warnings.append(source_plan_skipped_sources_warning)
        if loaded_source_plan is not None:
            manifest_payload["source_plan"] = loaded_source_plan.manifest_payload()
            build_payload["source_plan_digest"] = loaded_source_plan.digest
            build_payload["source_plan_skipped_generated_source_count"] = len(
                loaded_source_plan.skipped_generated_sources
            )
            if cython_regenerations:
                manifest_payload["cython_standalone"] = [
                    cython_regenerations[key].manifest_payload()
                    for key in sorted(cython_regenerations)
                ]
        build_payload["object_count"] = len(object_facts)
        build_payload["linked_object_count"] = len(object_paths)
        build_payload["object_closure_sha256"] = (
            source_plan_object_closure.closure_sha256
        )
        required_c_api_by_source = {
            fact.source_path.resolve(): tuple(
                sorted(
                    set(
                        source_c_api_requirements.required_by_source.get(
                            fact.source_path.resolve(), ()
                        )
                    )
                    | {
                        symbol
                        for symbol in fact.undefined_symbols
                        if is_c_api_external_requirement(symbol)
                    }
                )
            )
            for fact in source_plan_object_closure.objects
        }
        manifest_payload["object_closure"] = (
            source_plan_object_closure.manifest_payload(
                runtime_symbols=(
                    wasm_static_link_runtime_symbols_for_imports(
                        source_plan_object_closure.undefined_symbols
                    )
                    if wasm_static_link
                    else source_plan_object_closure.undefined_symbols
                ),
                required_c_api_by_source=required_c_api_by_source,
                required_capsules_by_source=(
                    source_c_api_requirements.required_capsules_by_source
                ),
                project_generated_c_api_by_source=(
                    source_c_api_requirements.project_generated_c_api_by_source
                ),
                project_generated_c_api_prefixes=(
                    source_c_api_requirements.project_generated_c_api_prefixes
                ),
            )
        )
        if python_exports:
            manifest_payload["python_exports"] = python_exports
        if callable_exports:
            manifest_payload["callable_exports"] = callable_exports
        if not support_files:
            manifest_payload.pop("support_files", None)
        wheel_identity_roots: list[tuple[Path | None, str]] = [
            (project_root, "@source"),
            (
                loaded_source_plan.build_root
                if loaded_source_plan is not None
                else None,
                "@build",
            ),
            (output_root, "@output"),
            (build_tmp, "@object-root"),
            (molt_root, "@molt"),
            (wasi_sysroot, "@wasi-sysroot"),
            (Path(sys.prefix), "@python"),
        ]
        for command in effective_tool_commands.values():
            if command:
                wheel_identity_roots.append(
                    (Path(command[0]).resolve().parent, "@toolchain")
                )
        wheel_manifest_payload = _canonical_extension_manifest_for_wheel(
            manifest_payload,
            location_roots=tuple(wheel_identity_roots),
            meson_plan_path=(
                loaded_source_plan.plan_path if loaded_source_plan is not None else None
            ),
            compile_commands_path=(
                loaded_source_plan.compile_commands_path
                if loaded_source_plan is not None
                else None
            ),
        )
        manifest_bytes = (
            json.dumps(wheel_manifest_payload, sort_keys=True, indent=2).encode("utf-8")
            + b"\n"
        )

        dist_info = f"{dist_name}-{wheel_version}.dist-info"
        wheel_metadata = "\n".join(
            [
                "Wheel-Version: 1.0",
                "Generator: molt extension build",
                "Root-Is-Purelib: false",
                f"Tag: {python_tag}-{abi_tag}-{platform_tag}",
                "",
            ]
        ).encode("utf-8")
        package_metadata = "\n".join(
            [
                "Metadata-Version: 2.1",
                f"Name: {project_name}",
                f"Version: {project_version}",
                "Summary: Molt C extension package",
                "",
            ]
        ).encode("utf-8")

        wheel_entries: list[tuple[str, bytes]] = [
            (extension_archive_path, extension_bytes),
            ("extension_manifest.json", manifest_bytes),
            (f"{dist_info}/WHEEL", wheel_metadata),
            (f"{dist_info}/METADATA", package_metadata),
        ]
        for support in support_files:
            wheel_entries.append((support.rel_path, support.source_path.read_bytes()))
        for link_input in link_requirements.inputs:
            wheel_entries.append(
                (
                    f"{module_parts[0]}/{link_input.path}",
                    (output_root / module_parts[0] / link_input.path).read_bytes(),
                )
            )
        record_path = f"{dist_info}/RECORD"
        _write_extension_wheel(
            wheel_path,
            entries=wheel_entries,
            record_path=record_path,
        )

    wheel_sha = _sha256_file(wheel_path)
    extension_sha = hashlib.sha256(extension_bytes).hexdigest()
    sidecar_payload = dict(manifest_payload)
    sidecar_payload["wheel_sha256"] = wheel_sha
    sidecar_payload["extension_sha256"] = extension_sha
    if deterministic:
        sidecar_payload["generated_at_utc"] = "1970-01-01T00:00:00Z"
    else:
        sidecar_payload["generated_at_utc"] = (
            dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
        )
    manifest_path = output_root / "extension_manifest.json"
    _atomic_write_json(manifest_path, sidecar_payload, sort_keys=True, indent=2)
    extracted_extension_path = output_root / extension_archive_path
    _atomic_write_bytes(extracted_extension_path, extension_bytes)
    extracted_package_init_files: list[Path] = []
    for index in range(1, len(module_parts)):
        source_init = project_root.joinpath(*module_parts[:index], "__init__.py")
        if not source_init.exists() or not source_init.is_file():
            continue
        dest_init = output_root.joinpath(*module_parts[:index], "__init__.py")
        _atomic_copy_file(source_init, dest_init)
        extracted_package_init_files.append(dest_init)
    extracted_support_files: list[Path] = []
    for support in support_files:
        dest_support = output_root / Path(support.rel_path)
        _atomic_copy_file(support.source_path, dest_support)
        extracted_support_files.append(dest_support)
    artifact_manifest_payload = dict(sidecar_payload)
    artifact_manifest_payload["extension"] = extracted_extension_path.name
    artifact_manifest_path = extracted_extension_path.with_name(
        extracted_extension_path.name + ".extension_manifest.json"
    )
    _atomic_write_json(
        artifact_manifest_path,
        artifact_manifest_payload,
        sort_keys=True,
        indent=2,
    )

    if json_output:
        payload = _json_payload(
            "extension-build",
            "ok",
            data={
                "project": str(project_root),
                "wheel": str(wheel_path),
                "manifest": str(manifest_path),
                "extension_artifact": str(extracted_extension_path),
                "artifact_manifest": str(artifact_manifest_path),
                "extracted_package_init_files": [
                    str(path) for path in extracted_package_init_files
                ],
                "extracted_support_files": [
                    str(path) for path in extracted_support_files
                ],
                "module": ".".join(module_parts),
                "molt_c_api_version": abi_version,
                "abi_tag": abi_tag,
                "abi_tier": normalized_abi_tier,
                "target_triple": target_triple,
                "target_python": extension_target_python.tag,
                "build_target": runtime_target_triple or "native",
                "platform_tag": platform_tag,
                "runtime_linkage": runtime_linkage,
                "artifact_kind": artifact_kind,
                "deterministic": deterministic,
                "determinism": determinism_mode,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "wheel_sha256": wheel_sha,
                "extension_sha256": extension_sha,
                "object_closure_sha256": (
                    source_plan_object_closure.closure_sha256
                    if source_plan_object_closure is not None
                    else None
                ),
                "object_count": len(object_facts),
                "linked_object_count": len(object_paths),
                "provided_capsules": list(provided_capsules_tuple),
                "source_plan_digest": (
                    loaded_source_plan.digest
                    if loaded_source_plan is not None
                    else None
                ),
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Built extension wheel: {wheel_path}")
        print(f"Wrote extension manifest: {manifest_path}")
        print(f"Wrote extension artifact: {extracted_extension_path}")
        print(f"Wrote artifact manifest: {artifact_manifest_path}")
        if source_plan_skipped_sources_warning is not None:
            print(f"Warning: {source_plan_skipped_sources_warning}", file=sys.stderr)
        if verbose:
            print(f"Target triple: {target_triple}")
            print(f"Build target: {runtime_target_triple or 'native'}")
            print(f"Molt C API version: {abi_version}")
            print(f"Extension ABI tier: {normalized_abi_tier}")
            print(f"Capabilities: {json.dumps(capabilities_list)}")
            print(f"Compile steps: {len(compile_commands)}")
            if source_plan_object_closure is not None:
                print(
                    "Source extension object closure: "
                    f"{len(source_plan_object_closure.objects)}/{len(object_facts)} "
                    "objects"
                )
            if extracted_package_init_files:
                print(
                    "Copied package init files: "
                    + ", ".join(str(path) for path in extracted_package_init_files)
                )
    return 0
