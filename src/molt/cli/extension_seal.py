from __future__ import annotations

import json
from pathlib import Path
from collections.abc import Mapping
from typing import Any

from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_json
from molt.cli.c_api_symbols import is_c_api_external_requirement
from molt.cli.extension_manifest import (
    ExtensionSupportFile,
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _manifest_support_file_payloads,
    _validate_extension_manifest,
)
from molt.cli.external_native import (
    _validate_module_attr_callable_export_custody,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.source_extensions import (
    source_extension_manifest_source_path,
    source_extension_manifest_required_capsule_imports_by_source,
)
from molt.wasm_artifact import read_wasm_function_exports


def _load_manifest(path: Path, errors: list[str]) -> dict[str, Any] | None:
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
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
    candidates = (
        (extension_path,)
        if extension_path.is_absolute()
        else (
            manifest_path.parent / extension_path,
            manifest_path.parent / extension_path.name,
        )
    )
    for candidate in candidates:
        if candidate.exists() and candidate.is_file():
            return candidate.resolve(), []
    return None, [f"extension artifact not found: {candidates[0]}"]


def _module_parts(manifest: Mapping[str, Any], errors: list[str]) -> tuple[str, ...]:
    module = manifest.get("module")
    if not isinstance(module, str) or not module.strip():
        errors.append("extension manifest has no valid module")
        return ()
    parts = tuple(part for part in module.strip().split(".") if part)
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
    manifest: Mapping[str, Any],
    manifest_path: Path,
    package: str,
) -> Path:
    source_package_root = manifest_path.parent
    package_index = None
    for index, part in enumerate(manifest_path.parent.parts):
        if part == package:
            package_index = index
            break
    if package_index is not None:
        return Path(*manifest_path.parent.parts[:package_index])
    package_parts = tuple(part for part in package.split(".") if part)
    raw_sources = manifest.get("sources")
    if isinstance(raw_sources, list):
        for raw_source in raw_sources:
            if not isinstance(raw_source, str) or not raw_source.strip():
                continue
            source_path = Path(raw_source).expanduser()
            if not source_path.is_absolute():
                source_path = (manifest_path.parent / source_path).resolve()
            else:
                source_path = source_path.resolve()
            parts = source_path.parts
            for index in range(0, len(parts) - len(package_parts) + 1):
                if tuple(parts[index : index + len(package_parts)]) == package_parts:
                    return Path(*parts[:index])
    return source_package_root


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
            _atomic_copy_file(support_file.source_path, dest)
        except OSError as exc:
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


def _validate_direct_symbol_exports(
    *,
    artifact_path: Path,
    manifest: Mapping[str, Any],
    callable_exports: list[dict[str, Any]],
) -> list[str]:
    if manifest.get("runtime_linkage") != "static_link":
        return []
    if manifest.get("artifact_kind") != "wasm_relocatable_object":
        return []
    direct_symbols = sorted(
        {
            symbol.strip()
            for export in callable_exports
            if export.get("binding") == "direct_symbol"
            and isinstance((symbol := export.get("symbol")), str)
            and symbol.strip()
        }
    )
    if not direct_symbols:
        return []
    try:
        exported_symbols = {
            export.name for export in read_wasm_function_exports(artifact_path)
        }
    except (OSError, UnicodeDecodeError, ValueError, IndexError) as exc:
        return [f"cannot validate direct_symbol callable exports: {exc}"]
    missing = [symbol for symbol in direct_symbols if symbol not in exported_symbols]
    if not missing:
        return []
    return [
        "direct_symbol callable export(s) absent from wasm function exports: "
        + ", ".join(missing)
    ]


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


def _string_set(value: Any) -> set[str]:
    if not isinstance(value, list):
        return set()
    return {item.strip() for item in value if isinstance(item, str) and item.strip()}


def _canonicalize_object_closure_source_capsule_requirements(
    manifest: dict[str, Any],
    *,
    manifest_path: Path,
) -> list[str]:
    by_source, errors = source_extension_manifest_required_capsule_imports_by_source(
        manifest,
        manifest_path=manifest_path,
    )
    if errors:
        return errors
    assert by_source is not None
    if not by_source:
        return []
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, dict):
        return ["extension seal requires non-empty object_closure custody"]
    required_capsules = _string_set(object_closure.get("required_capsules"))
    for imports_by_capsule in by_source.values():
        required_capsules.update(imports_by_capsule)
    object_closure["required_capsules"] = sorted(required_capsules)

    objects = object_closure.get("objects")
    if not isinstance(objects, list):
        return []
    for item in objects:
        if not isinstance(item, dict):
            continue
        source = item.get("source")
        if not isinstance(source, str) or not source.strip():
            continue
        source_sha256 = item.get("source_sha256")
        source_path, source_errors = source_extension_manifest_source_path(
            source,
            manifest=manifest,
            manifest_path=manifest_path,
            expected_sha256=(
                source_sha256.strip()
                if isinstance(source_sha256, str) and source_sha256.strip()
                else None
            ),
        )
        if source_errors:
            return source_errors
        if source_path is None:
            continue
        imports_by_capsule = by_source.get(source_path)
        if not imports_by_capsule:
            continue
        item_capsules = _string_set(item.get("required_capsules"))
        item_capsules.update(imports_by_capsule)
        item["required_capsules"] = sorted(item_capsules)
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
    source_package_root = _source_package_root_for_manifest(
        manifest=manifest,
        manifest_path=manifest_path,
        package=package,
    )
    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=manifest_path.parent,
        wheel_path=None,
        require_capabilities=True,
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
    if not isinstance(object_closure, Mapping) or not object_closure:
        errors.append("extension seal requires non-empty object_closure custody")
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

    raw_python_exports = (
        list(python_export or [])
        if python_export
        else list(manifest.get("python_exports") or [])
    )
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
    if manifest_support_files is not None and not support_file:
        if isinstance(manifest_support_files, list):
            raw_support_files.extend(manifest_support_files)
        else:
            support_sha_errors.append("support_files must be a list when present")
    raw_support_files.extend(
        _support_file_payloads(support_file, errors=support_sha_errors)
    )
    support_files = _manifest_support_file_payloads(
        raw_support_files,
        field_name="support_files",
        root=source_package_root,
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
    errors.extend(
        _validate_direct_symbol_exports(
            artifact_path=artifact_path,
            manifest=manifest,
            callable_exports=callable_exports,
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

    sealed_manifest = dict(manifest)
    _canonicalize_object_closure_c_api_requirements(sealed_manifest)
    source_capsule_errors = _canonicalize_object_closure_source_capsule_requirements(
        sealed_manifest,
        manifest_path=manifest_path,
    )
    if source_capsule_errors:
        return _fail(
            "; ".join(source_capsule_errors),
            json_output,
            command="extension-seal",
        )

    output_root = Path(out_dir).expanduser()
    if not output_root.is_absolute():
        output_root = (Path.cwd() / output_root).absolute()
    dest_artifact_rel = Path(*module_parts[:-1], artifact_path.name)
    dest_artifact_path = output_root / dest_artifact_rel
    _atomic_copy_file(artifact_path, dest_artifact_path)
    copied_inits = _copy_package_init_chain(
        module_parts=module_parts,
        source_package_root=source_package_root,
        output_root=output_root,
    )
    copied_support_files, support_errors = _copy_support_files(
        support_files,
        output_root=output_root,
    )
    if support_errors:
        return _fail(
            "; ".join(support_errors),
            json_output,
            command="extension-seal",
        )

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

    root_manifest = dict(sealed_manifest)
    root_manifest["extension"] = dest_artifact_rel.as_posix()
    artifact_manifest = dict(sealed_manifest)
    artifact_manifest["extension"] = dest_artifact_path.name
    root_manifest_path = output_root / "extension_manifest.json"
    artifact_manifest_path = dest_artifact_path.with_name(
        dest_artifact_path.name + ".extension_manifest.json"
    )
    _atomic_write_json(root_manifest_path, root_manifest, sort_keys=True, indent=2)
    _atomic_write_json(
        artifact_manifest_path,
        artifact_manifest,
        sort_keys=True,
        indent=2,
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
