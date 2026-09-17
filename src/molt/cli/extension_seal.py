from __future__ import annotations

import json
from pathlib import Path
from collections.abc import Mapping
from typing import Any, cast

from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_json
from molt.c_api_symbols import is_c_api_external_requirement
from molt.cli.extension_manifest import (
    ExtensionSupportFile,
    _default_molt_c_api_version,
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _module_parts as _extension_module_parts,
    _validate_extension_manifest,
)
from molt.cli.extension_support import module_attr_support_files
from molt.python_module_names import encode_python_module_names
from molt.cli.project_roots import _find_molt_root
from molt.cli.external_native import (
    _manifest_has_sealed_extension_custody,
    _validate_module_attr_callable_export_custody,
)
from molt.file_hashing import _sha256_file
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.source_extensions import (
    canonicalize_source_extension_manifest_required_capsules,
    canonicalize_source_extension_manifest_runtime_python_imports,
    validate_source_extension_artifact_object_closure,
)
from molt.cli.source_extension_link_requirements import (
    materialize_source_extension_link_requirements,
    parse_source_extension_link_requirements,
)
from molt.cli.source_extension_input_custody import (
    SourceExtensionInputCustodyError,
    project_source_extension_manifest_inputs,
    stage_source_extension_manifest_inputs,
)
from molt.cli.source_extension_manifest_codec import (
    _compact_source_extension_manifest,
    _expand_source_extension_manifest_authorities,
    _validate_compact_source_extension_manifest,
)
from molt.cli.source_extension_object_closure import (
    SourceExtensionObjectClosureError,
    validate_source_extension_object_closure,
    validate_source_extension_object_closure_sources,
)
from molt.target_python import _parse_target_python_version


def _load_manifest(path: Path, errors: list[str]) -> dict[str, Any] | None:
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        errors.append(f"Failed to read extension manifest {path}: {exc}")
        return None
    if not isinstance(loaded, dict):
        errors.append(f"Extension manifest must be a JSON object: {path}")
        return None
    return loaded


def _resolve_manifest_path(path: str) -> tuple[Path | None, list[str]]:
    target = Path(path).expanduser()
    if not target.is_absolute():
        target = (Path.cwd() / target).absolute()
    if target.is_dir():
        manifest_path = target / "extension_manifest.json"
        if not manifest_path.exists():
            return None, [f"Missing extension manifest: {manifest_path}"]
        return manifest_path, []
    if target.is_file() and target.suffix == ".json":
        return target, []
    return None, [f"Unsupported seal path: {target}"]


def _resolve_declared_artifact(
    *,
    manifest: Mapping[str, Any],
    manifest_path: Path,
) -> tuple[Path | None, list[str]]:
    extension = manifest.get("extension")
    if not isinstance(extension, str) or not extension.strip():
        return None, ["extension artifact path missing"]
    extension_path = Path(extension.strip()).expanduser()
    if not extension_path.is_absolute():
        extension_path = manifest_path.parent / extension_path
    if extension_path.is_file():
        return extension_path.resolve(), []
    return None, [f"extension artifact not found: {extension_path}"]


def _module_parts(manifest: Mapping[str, Any], errors: list[str]) -> tuple[str, ...]:
    module = manifest.get("module")
    if not isinstance(module, str) or not module.strip():
        errors.append("extension manifest has no valid module")
        return ()
    parsed_parts = _extension_module_parts(module)
    if parsed_parts is None:
        errors.append("extension manifest has no canonical extension module")
        return ()
    parts = tuple(parsed_parts)
    if len(parts) < 2:
        errors.append("extension manifest module must name a package child module")
        return ()
    return parts


def _copy_package_init_chain(
    *,
    module_parts: tuple[str, ...],
    source_package_root: Path,
    output_root: Path,
) -> list[Path]:
    copied: list[Path] = []
    for index in range(1, len(module_parts)):
        source_init = source_package_root.joinpath(*module_parts[:index], "__init__.py")
        if not source_init.exists() or not source_init.is_file():
            continue
        dest_init = output_root.joinpath(*module_parts[:index], "__init__.py")
        _atomic_copy_file(source_init, dest_init)
        copied.append(dest_init)
    return copied


def _source_package_root_for_manifest(
    *,
    artifact_path: Path,
    module_parts: tuple[str, ...],
) -> Path:
    """Derive package custody from the exact module-relative artifact layout."""
    parents = module_parts[:-1]
    if artifact_path.parent.parts[-len(parents) :] != parents:
        raise ValueError(
            "extension artifact does not occupy its declared module layout: "
            f"{'.'.join(module_parts)}: {artifact_path}"
        )
    return artifact_path.parents[len(parents)]


def _copy_support_files(
    support_files: tuple[ExtensionSupportFile, ...],
    *,
    output_root: Path,
) -> tuple[list[Path], list[str]]:
    copied: list[Path] = []
    errors: list[str] = []
    for support_file in support_files:
        dest = output_root / Path(support_file.rel_path)
        try:
            _atomic_copy_file(
                support_file.source_path, dest, expected_sha256=support_file.sha256
            )
        except (OSError, ValueError) as exc:
            errors.append(
                f"support file copy failed for {support_file.rel_path}: {exc}"
            )
            continue
        copied.append(dest)
    return copied, errors


def _callable_exports(
    *,
    package: str,
    existing_manifest: Mapping[str, Any],
    callable_export_json: list[str] | None,
    errors: list[str],
) -> tuple[Any, ...]:
    raw_exports = (
        []
        if callable_export_json
        else list(existing_manifest.get("callable_exports") or [])
    )
    for index, item in enumerate(callable_export_json or []):
        try:
            payload = json.loads(item)
        except json.JSONDecodeError as exc:
            errors.append(f"--callable-export-json[{index}] must be JSON: {exc}")
            continue
        if not isinstance(payload, dict):
            errors.append(f"--callable-export-json[{index}] must be a JSON object")
            continue
        raw_exports.append(payload)
    if not raw_exports:
        return ()
    export_errors: list[str] = []
    exports = _manifest_callable_exports(
        {"callable_exports": raw_exports},
        package=package,
        errors=export_errors,
    )
    errors.extend(export_errors)
    return tuple(exports)


def _support_file_payloads(
    support_file: list[Any] | str | None,
    *,
    errors: list[str],
) -> list[Any]:
    if support_file is None:
        return []
    raw_items: list[Any]
    if isinstance(support_file, str):
        raw_items = [support_file]
    elif isinstance(support_file, list):
        raw_items = list(support_file)
    else:
        errors.append("--support-file must be a string or list")
        return []

    payloads: list[Any] = []
    for index, item in enumerate(raw_items):
        if not isinstance(item, str):
            payloads.append(item)
            continue
        stripped = item.strip()
        if not stripped.startswith("{"):
            payloads.append(item)
            continue
        try:
            parsed = json.loads(stripped)
        except json.JSONDecodeError as exc:
            errors.append(
                f"--support-file[{index}] must be a path or JSON object: {exc}"
            )
            continue
        if not isinstance(parsed, dict):
            errors.append(f"--support-file[{index}] JSON must be an object")
            continue
        payloads.append(parsed)
    return payloads


def _canonicalize_object_closure_c_api_requirements(
    manifest: dict[str, Any],
) -> None:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, dict):
        return
    for owner in (object_closure,):
        raw_symbols = owner.get("required_c_api_symbols")
        if isinstance(raw_symbols, list):
            owner["required_c_api_symbols"] = sorted(
                {
                    symbol.strip()
                    for symbol in raw_symbols
                    if isinstance(symbol, str)
                    and is_c_api_external_requirement(symbol.strip())
                }
            )
    objects = object_closure.get("objects")
    if not isinstance(objects, list):
        return
    for item in objects:
        if not isinstance(item, dict):
            continue
        item = cast(dict[str, Any], item)
        raw_symbols = item.get("required_c_api_symbols")
        if not isinstance(raw_symbols, list):
            continue
        item["required_c_api_symbols"] = sorted(
            {
                symbol.strip()
                for symbol in raw_symbols
                if isinstance(symbol, str)
                and is_c_api_external_requirement(symbol.strip())
            }
        )


def _sealed_manifest_projection(
    manifest: dict[str, Any],
    *,
    source_manifest_path: Path,
    output_manifest_path: Path,
    output_root: Path,
    staged_inputs: Mapping[Path, Path],
) -> dict[str, Any]:
    """Project retained inputs into one sidecar and finalize its exact identity."""
    projected = project_source_extension_manifest_inputs(
        manifest,
        source_manifest_path=source_manifest_path,
        output_manifest_path=output_manifest_path,
        publish_root=output_root,
        staged_inputs=staged_inputs,
    )
    projected = _compact_source_extension_manifest(projected)
    _validate_compact_source_extension_manifest(projected)
    validate_source_extension_object_closure(projected)
    return projected


def _restamp_current_runtime_abi(
    sealed_manifest: dict[str, Any],
    *,
    molt_root: Path,
) -> list[str]:
    """Stamp the sealed manifest with the current runtime's C-API version.

    Seal is the custody boundary that admits a recompiled native artifact into
    the current runtime. The runtime header ``MOLT_C_API_VERSION`` is the single
    authority for the ABI the artifact will link against, so the sealed manifest
    must carry that version rather than propagating a stale build-time label.

    The Molt extension ABI is a monotonically forward-compatible stable core
    (opaque ``MoltHandle`` object model plus additively-versioned ``molt_*``
    runtime symbols), so an artifact recorded at an older major ABI is
    admissible under a newer runtime. An artifact recorded at a *newer* major
    ABI than the current runtime is a genuine mismatch and fails closed.
    """
    current = _default_molt_c_api_version(molt_root)
    try:
        current_major = int(current.split(".", 1)[0])
    except (ValueError, IndexError):
        return [f"cannot determine current runtime C-API major from {current!r}"]
    manifest_abi = sealed_manifest.get("molt_c_api_version")
    if isinstance(manifest_abi, str) and manifest_abi.strip():
        try:
            manifest_major = int(manifest_abi.strip().split(".", 1)[0])
        except ValueError:
            return [
                "cannot determine manifest C-API major from "
                f"{manifest_abi!r} before sealing to current runtime ABI"
            ]
        if manifest_major > current_major:
            return [
                "extension manifest requires C-API major "
                f"{manifest_major}, newer than the current runtime "
                f"{current_major}; rebuild the extension against this runtime "
                "before sealing"
            ]
    sealed_manifest["molt_c_api_version"] = current
    sealed_manifest["abi_tag"] = f"molt_abi{current_major}"
    return []


def extension_seal(
    path: str,
    out_dir: str,
    python_export: list[str] | None = None,
    callable_export_json: list[str] | None = None,
    support_file: list[Any] | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    manifest_path, path_errors = _resolve_manifest_path(path)
    if path_errors:
        return _fail(
            "; ".join(path_errors),
            json_output,
            command="extension-seal",
        )
    assert manifest_path is not None
    errors: list[str] = []
    warnings: list[str] = []
    manifest = _load_manifest(manifest_path, errors)
    if manifest is None:
        return _fail(
            "; ".join(errors),
            json_output,
            command="extension-seal",
        )
    artifact_path, artifact_errors = _resolve_declared_artifact(
        manifest=manifest,
        manifest_path=manifest_path,
    )
    errors.extend(artifact_errors)
    module_parts = _module_parts(manifest, errors)
    if errors:
        return _fail(
            "; ".join(errors),
            json_output,
            command="extension-seal",
        )
    assert artifact_path is not None
    package = module_parts[0]
    module_name = ".".join(module_parts)
    try:
        source_package_root = _source_package_root_for_manifest(
            artifact_path=artifact_path,
            module_parts=module_parts,
        )
    except ValueError as exc:
        return _fail(str(exc), json_output, command="extension-seal")
    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=manifest_path.parent,
        wheel_path=None,
        require_nonempty_capabilities=False,
        required_abi=None,
        require_checksum=False,
        warn_missing_checksum=False,
        allow_missing_wheel=True,
    )
    errors.extend(validation.errors)
    if manifest.get("loader_kind") != "libmolt_source":
        errors.append("extension seal requires loader_kind 'libmolt_source'")
    if manifest.get("runtime_linkage") != "static_link":
        errors.append("extension seal requires runtime_linkage 'static_link'")
    if manifest.get("artifact_kind") not in {
        "wasm_relocatable_object",
        "static_archive",
    }:
        errors.append(
            "extension seal requires artifact_kind 'wasm_relocatable_object' "
            "or 'static_archive'"
        )
    object_closure = manifest.get("object_closure")
    validated_object_closure: tuple[dict[str, Any], str] | None = None
    if not isinstance(object_closure, Mapping) or not object_closure:
        errors.append("extension seal requires non-empty object_closure custody")
    else:
        try:
            validated_object_closure = validate_source_extension_object_closure(
                manifest
            )
            if not _manifest_has_sealed_extension_custody(manifest):
                validate_source_extension_object_closure_sources(
                    object_closure,
                    manifest_dir=manifest_path.parent,
                    manifest=manifest,
                )
        except SourceExtensionObjectClosureError as exc:
            errors.append(f"extension seal object closure is invalid: {exc}")
    expected_extension_sha = manifest.get("extension_sha256")
    actual_extension_sha = _sha256_file(artifact_path)
    if (
        isinstance(expected_extension_sha, str)
        and expected_extension_sha.strip()
        and expected_extension_sha.strip() != actual_extension_sha
    ):
        errors.append(
            "extension_sha256 does not match extension artifact; rebuild or "
            "repair artifact custody before sealing exports"
        )
    if (
        not isinstance(expected_extension_sha, str)
        or not expected_extension_sha.strip()
    ):
        errors.append("extension seal requires existing extension_sha256 custody")

    raw_target_python = manifest.get("target_python")
    if not isinstance(raw_target_python, str) or not raw_target_python.strip():
        errors.append("extension seal requires target_python custody")
        target_python = None
    else:
        try:
            target_python = _parse_target_python_version(raw_target_python)
        except ValueError as exc:
            errors.append(str(exc))
            target_python = None
    if target_python is None:
        return _fail("; ".join(errors), json_output, command="extension-seal")

    raw_python_exports = manifest.get("python_exports", [])
    if python_export is not None:
        try:
            raw_python_exports = encode_python_module_names(
                python_export, field="--python-export"
            )
        except ValueError as exc:
            errors.append(str(exc))
    export_manifest = {"python_exports": raw_python_exports}
    python_export_errors: list[str] = []
    python_exports = _manifest_dotted_name_tuple(
        export_manifest,
        "python_exports",
        package=package,
        errors=python_export_errors,
    )
    errors.extend(python_export_errors)
    callable_export_specs = _callable_exports(
        package=package,
        existing_manifest=manifest,
        callable_export_json=callable_export_json,
        errors=errors,
    )
    callable_exports = [export.digest_payload() for export in callable_export_specs]
    support_sha_errors: list[str] = []
    manifest_support_files = manifest.get("support_files")
    raw_support_files: list[Any] = []
    if (
        manifest_support_files is not None
        and not support_file
        and not callable_export_json
    ):
        if isinstance(manifest_support_files, list):
            raw_support_files.extend(manifest_support_files)
        else:
            support_sha_errors.append("support_files must be a list when present")
    raw_support_files.extend(
        _support_file_payloads(support_file, errors=support_sha_errors)
    )
    support_files = module_attr_support_files(
        raw_support_files,
        field_name="support_files",
        source_root=source_package_root,
        package=package,
        extension_module=module_name,
        callable_exports=callable_exports,
        target_python=target_python,
        errors=support_sha_errors,
    )
    support_file_sha256 = tuple(
        (entry.rel_path, entry.sha256) for entry in support_files
    )
    errors.extend(support_sha_errors)
    errors.extend(
        _validate_module_attr_callable_export_custody(
            package=package,
            manifest=manifest,
            manifest_path=manifest_path,
            module_name=module_name,
            callable_exports=callable_export_specs,
            support_file_sha256=support_file_sha256,
        )
    )
    direct_symbols = tuple(
        sorted(
            {
                symbol.strip()
                for export in callable_exports
                if export.get("binding") == "direct_symbol"
                and isinstance((symbol := export.get("symbol")), str)
                and symbol.strip()
            }
        )
    )
    if validated_object_closure is not None:
        errors.extend(
            validate_source_extension_artifact_object_closure(
                artifact_path=artifact_path,
                manifest=manifest,
                required_function_exports=direct_symbols,
                validated_closure=validated_object_closure,
            )
        )
    if not python_exports and not callable_exports:
        errors.append(
            "extension seal requires at least one python export or callable export"
        )
    if errors:
        return _fail(
            "; ".join(errors),
            json_output,
            command="extension-seal",
        )

    try:
        sealed_manifest = _expand_source_extension_manifest_authorities(manifest)
    except ValueError as exc:
        return _fail(
            f"extension seal cannot materialize manifest authority: {exc}",
            json_output,
            command="extension-seal",
        )
    abi_restamp_errors = _restamp_current_runtime_abi(
        sealed_manifest,
        molt_root=_find_molt_root(manifest_path.parent, Path.cwd()),
    )
    if abi_restamp_errors:
        return _fail(
            "; ".join(abi_restamp_errors),
            json_output,
            command="extension-seal",
        )
    _canonicalize_object_closure_c_api_requirements(sealed_manifest)
    source_capsule_errors = canonicalize_source_extension_manifest_required_capsules(
        sealed_manifest,
        manifest_path=manifest_path,
    )
    if source_capsule_errors:
        return _fail(
            "; ".join(source_capsule_errors),
            json_output,
            command="extension-seal",
        )
    runtime_import_errors = (
        canonicalize_source_extension_manifest_runtime_python_imports(
            sealed_manifest,
            manifest_path=manifest_path,
        )
    )
    if runtime_import_errors:
        return _fail(
            "; ".join(runtime_import_errors),
            json_output,
            command="extension-seal",
        )
    output_root = Path(out_dir).expanduser()
    if not output_root.is_absolute():
        output_root = (Path.cwd() / output_root).absolute()
    try:
        staged_inputs = stage_source_extension_manifest_inputs(
            sealed_manifest,
            manifest_path=manifest_path,
            publish_root=output_root,
        )
    except (SourceExtensionInputCustodyError, OSError) as exc:
        return _fail(
            f"extension seal cannot retain compilation inputs: {exc}",
            json_output,
            command="extension-seal",
        )
    target_triple = sealed_manifest.get("target_triple")
    assert isinstance(target_triple, str)
    link_requirements, link_requirement_errors = (
        parse_source_extension_link_requirements(
            sealed_manifest,
            expected_target_triple=target_triple,
        )
    )
    if link_requirement_errors:
        return _fail(
            "; ".join(link_requirement_errors),
            json_output,
            command="extension-seal",
        )
    assert link_requirements is not None
    materialized_link_requirements, link_materialization_errors = (
        materialize_source_extension_link_requirements(
            link_requirements,
            package_root=source_package_root,
            manifest_dir=manifest_path.parent,
            publish_root=output_root / package,
        )
    )
    if link_materialization_errors:
        return _fail(
            "; ".join(link_materialization_errors),
            json_output,
            command="extension-seal",
        )
    assert materialized_link_requirements is not None
    sealed_manifest["link_requirements"] = (
        materialized_link_requirements.manifest_payload()
    )
    dest_artifact_rel = Path(*module_parts[:-1], artifact_path.name)
    dest_artifact_path = output_root / dest_artifact_rel

    sealed_manifest["python_exports"] = list(python_exports)
    if support_files:
        sealed_manifest["support_files"] = [
            entry.digest_payload() for entry in support_files
        ]
    else:
        sealed_manifest.pop("support_files", None)
    if callable_exports:
        sealed_manifest["callable_exports"] = callable_exports
    elif "callable_exports" in sealed_manifest:
        sealed_manifest.pop("callable_exports")
    sealed_manifest["extension_sha256"] = actual_extension_sha
    sealed_manifest["sealed_from_manifest_sha256"] = _sha256_file(manifest_path)
    sealed_manifest["sealed_from_extension_sha256"] = actual_extension_sha
    root_manifest_path = output_root / "extension_manifest.json"
    artifact_manifest_path = dest_artifact_path.with_name(
        dest_artifact_path.name + ".extension_manifest.json"
    )
    try:
        root_manifest = _sealed_manifest_projection(
            sealed_manifest,
            source_manifest_path=manifest_path,
            output_manifest_path=root_manifest_path,
            output_root=output_root,
            staged_inputs=staged_inputs,
        )
        artifact_manifest = _sealed_manifest_projection(
            sealed_manifest,
            source_manifest_path=manifest_path,
            output_manifest_path=artifact_manifest_path,
            output_root=output_root,
            staged_inputs=staged_inputs,
        )
    except (SourceExtensionObjectClosureError, ValueError) as exc:
        return _fail(
            f"extension seal cannot finalize object closure: {exc}",
            json_output,
            command="extension-seal",
        )

    try:
        _atomic_copy_file(
            artifact_path, dest_artifact_path, expected_sha256=actual_extension_sha
        )
        copied_inits = _copy_package_init_chain(
            module_parts=module_parts,
            source_package_root=source_package_root,
            output_root=output_root,
        )
    except (OSError, ValueError) as exc:
        return _fail(
            f"extension seal cannot publish artifact files: {exc}",
            json_output,
            command="extension-seal",
        )
    copied_support_files, support_errors = _copy_support_files(
        support_files,
        output_root=output_root,
    )
    if support_errors:
        return _fail("; ".join(support_errors), json_output, command="extension-seal")

    root_manifest["extension"] = dest_artifact_rel.as_posix()
    artifact_manifest["extension"] = dest_artifact_path.name
    try:
        _atomic_write_json(root_manifest_path, root_manifest, sort_keys=True, indent=2)
        _atomic_write_json(
            artifact_manifest_path,
            artifact_manifest,
            sort_keys=True,
            indent=2,
        )
    except OSError as exc:
        return _fail(
            f"extension seal cannot publish manifest custody: {exc}",
            json_output,
            command="extension-seal",
        )
    if json_output:
        payload = _json_payload(
            "extension-seal",
            "ok",
            data={
                "source_manifest": str(manifest_path),
                "source_artifact": str(artifact_path),
                "output_root": str(output_root),
                "manifest": str(root_manifest_path),
                "artifact_manifest": str(artifact_manifest_path),
                "extension_artifact": str(dest_artifact_path),
                "extension_sha256": actual_extension_sha,
                "python_exports": list(python_exports),
                "callable_exports": [
                    f"{export['module']}.{export['name']}"
                    for export in callable_exports
                ],
                "copied_package_init_files": [str(path) for path in copied_inits],
                "copied_support_files": [str(path) for path in copied_support_files],
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Extension sealed: {output_root}")
        if verbose:
            print(f"Manifest: {root_manifest_path}")
            print(f"Artifact manifest: {artifact_manifest_path}")
            print(f"Extension artifact: {dest_artifact_path}")
            for warning in warnings:
                print(f"WARN: {warning}")
    return 0
