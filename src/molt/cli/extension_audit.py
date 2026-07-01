from __future__ import annotations

import json
from pathlib import Path
from typing import Any
import zipfile
from collections.abc import Mapping

from molt.cli.extension_manifest import (
    _MOLT_C_API_VERSION_RE,
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _validate_extension_manifest,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload


def _object_closure_string_set(
    object_closure: Mapping[str, Any],
    field_name: str,
) -> set[str]:
    symbols = {
        item.strip()
        for item in object_closure.get(field_name, ())
        if isinstance(item, str) and item.strip()
    }
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if not isinstance(item, Mapping):
                continue
            symbols.update(
                value.strip()
                for value in item.get(field_name, ())
                if isinstance(value, str) and value.strip()
            )
    return symbols


def extension_audit(
    path: str,
    require_capabilities: bool = False,
    require_abi: str | None = None,
    require_checksum: bool = False,
    require_loader_kind: str | None = None,
    require_runtime_linkage: str | None = None,
    require_artifact_kind: str | None = None,
    require_artifact_file: bool = False,
    require_object_closure: bool = False,
    require_python_export: list[str] | None = None,
    require_callable_export: list[str] | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    target = Path(path).expanduser()
    if not target.is_absolute():
        target = (Path.cwd() / target).absolute()

    if (
        require_abi is not None
        and _MOLT_C_API_VERSION_RE.match(require_abi.strip()) is None
    ):
        return _fail(
            "Invalid --require-abi value. Expected MAJOR[.MINOR[.PATCH]].",
            json_output,
            command="extension-audit",
        )
    required_abi = require_abi.strip() if require_abi is not None else None
    required_loader_kind = (
        require_loader_kind.strip() if require_loader_kind is not None else None
    )
    required_runtime_linkage = (
        require_runtime_linkage.strip() if require_runtime_linkage is not None else None
    )
    required_artifact_kind = (
        require_artifact_kind.strip() if require_artifact_kind is not None else None
    )
    for flag_name, value in (
        ("--require-loader-kind", required_loader_kind),
        ("--require-runtime-linkage", required_runtime_linkage),
        ("--require-artifact-kind", required_artifact_kind),
    ):
        if value == "":
            return _fail(
                f"Invalid {flag_name} value. Expected a non-empty manifest token.",
                json_output,
                command="extension-audit",
            )

    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    manifest_source = ""
    manifest_dir = target.parent if target.is_file() else target
    wheel_path: Path | None = None

    def load_manifest_json(source_path: Path) -> dict[str, Any] | None:
        try:
            loaded = json.loads(source_path.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            errors.append(f"Failed to read extension manifest {source_path}: {exc}")
            return None
        if not isinstance(loaded, dict):
            errors.append(f"Extension manifest must be a JSON object: {source_path}")
            return None
        return loaded

    if target.is_dir():
        manifest_path = target / "extension_manifest.json"
        if not manifest_path.exists():
            return _fail(
                f"Missing extension manifest: {manifest_path}",
                json_output,
                command="extension-audit",
            )
        manifest = load_manifest_json(manifest_path)
        manifest_source = str(manifest_path)
        manifest_dir = manifest_path.parent
    elif target.is_file() and target.suffix == ".whl":
        wheel_path = target
        sibling_manifest = target.parent / "extension_manifest.json"
        if sibling_manifest.exists():
            manifest = load_manifest_json(sibling_manifest)
            manifest_source = str(sibling_manifest)
            manifest_dir = sibling_manifest.parent
        else:
            try:
                with zipfile.ZipFile(target) as zf:
                    manifest_bytes = zf.read("extension_manifest.json")
            except KeyError:
                return _fail(
                    "extension_manifest.json not found next to wheel or inside wheel.",
                    json_output,
                    command="extension-audit",
                )
            except (OSError, zipfile.BadZipFile) as exc:
                return _fail(
                    f"Failed to inspect wheel {target}: {exc}",
                    json_output,
                    command="extension-audit",
                )
            try:
                decoded = json.loads(manifest_bytes.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                return _fail(
                    f"Invalid embedded extension_manifest.json: {exc}",
                    json_output,
                    command="extension-audit",
                )
            if not isinstance(decoded, dict):
                return _fail(
                    "Embedded extension_manifest.json must be a JSON object.",
                    json_output,
                    command="extension-audit",
                )
            manifest = decoded
            manifest_source = f"{target}!/extension_manifest.json"
            manifest_dir = target.parent
    elif target.is_file() and target.suffix == ".json":
        manifest = load_manifest_json(target)
        manifest_source = str(target)
        manifest_dir = target.parent
    else:
        return _fail(
            f"Unsupported audit path: {target}",
            json_output,
            command="extension-audit",
        )

    if manifest is None:
        return _fail(
            "Failed to load extension manifest.",
            json_output,
            command="extension-audit",
        )

    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=manifest_dir,
        wheel_path=wheel_path,
        require_capabilities=require_capabilities,
        required_abi=required_abi,
        require_checksum=require_checksum and not require_artifact_file,
        warn_missing_checksum=not require_checksum and not require_artifact_file,
        allow_missing_wheel=require_artifact_file,
    )
    errors.extend(validation.errors)
    warnings.extend(validation.warnings)
    wheel_path = validation.wheel_path
    manifest_abi = validation.abi_version
    manifest_abi_tag = validation.abi_tag
    manifest_capabilities = validation.capabilities
    wheel_tags = validation.wheel_tags
    loader_kind = manifest.get("loader_kind")
    runtime_linkage = manifest.get("runtime_linkage")
    artifact_kind = manifest.get("artifact_kind")
    if required_loader_kind is not None and loader_kind != required_loader_kind:
        errors.append(
            f"loader_kind mismatch: required {required_loader_kind!r}, "
            f"manifest has {loader_kind!r}"
        )
    if (
        required_runtime_linkage is not None
        and runtime_linkage != required_runtime_linkage
    ):
        errors.append(
            f"runtime_linkage mismatch: required {required_runtime_linkage!r}, "
            f"manifest has {runtime_linkage!r}"
        )
    if required_artifact_kind is not None and artifact_kind != required_artifact_kind:
        errors.append(
            f"artifact_kind mismatch: required {required_artifact_kind!r}, "
            f"manifest has {artifact_kind!r}"
        )

    extension_entry = manifest.get("extension")
    extension_path: Path | None = None
    extension_file_status = "not-declared"
    if isinstance(extension_entry, str) and extension_entry.strip():
        candidate = Path(extension_entry.strip()).expanduser()
        if not candidate.is_absolute():
            candidate = (manifest_dir / candidate).absolute()
        extension_path = candidate
        if not candidate.exists():
            extension_file_status = "missing"
            if require_artifact_file:
                errors.append(f"extension artifact not found: {candidate}")
            elif wheel_path is None:
                warnings.append(
                    "Standalone extension artifact not found; pass "
                    "--require-artifact-file to fail closed for static-link custody."
                )
        elif not candidate.is_file():
            extension_file_status = "not-file"
            errors.append(f"extension artifact is not a regular file: {candidate}")
        else:
            expected_extension_sha = manifest.get("extension_sha256")
            if (
                isinstance(expected_extension_sha, str)
                and expected_extension_sha.strip()
            ):
                actual_extension_sha = _sha256_file(candidate)
                if actual_extension_sha != expected_extension_sha.strip():
                    extension_file_status = "sha256-mismatch"
                    errors.append("extension_sha256 does not match extension artifact")
                else:
                    extension_file_status = "ok"
            elif require_checksum:
                extension_file_status = "missing-sha256"
                errors.append("extension_sha256 missing")
            else:
                extension_file_status = "present-unverified"
                warnings.append("extension_sha256 missing")
    elif require_artifact_file:
        errors.append("extension artifact path missing")

    object_closure = manifest.get("object_closure")
    object_closure_summary = {
        "present": isinstance(object_closure, Mapping),
        "has_closure_sha256": False,
        "object_count": 0,
        "runtime_symbol_count": 0,
        "undefined_symbol_count": 0,
        "defined_symbol_count": 0,
        "required_c_api_symbol_count": 0,
        "required_capsule_count": 0,
        "project_generated_c_api_symbol_count": 0,
        "project_generated_c_api_prefix_count": 0,
        "root_symbol": None,
        "init_symbol_owner": None,
        "keys": [],
    }
    if isinstance(object_closure, Mapping):
        runtime_symbols = _object_closure_string_set(object_closure, "runtime_symbols")
        undefined_symbols = _object_closure_string_set(
            object_closure,
            "undefined_symbols",
        )
        defined_symbols = _object_closure_string_set(object_closure, "defined_symbols")
        required_c_api_symbols = _object_closure_string_set(
            object_closure,
            "required_c_api_symbols",
        )
        required_capsules = _object_closure_string_set(
            object_closure,
            "required_capsules",
        )
        project_generated_c_api_symbols = _object_closure_string_set(
            object_closure,
            "project_generated_c_api_symbols",
        )
        project_generated_c_api_prefixes = _object_closure_string_set(
            object_closure,
            "project_generated_c_api_prefixes",
        )
        object_closure_summary = {
            "present": True,
            "has_closure_sha256": isinstance(object_closure.get("closure_sha256"), str)
            and bool(object_closure.get("closure_sha256")),
            "object_count": len(object_closure.get("objects") or [])
            if isinstance(object_closure.get("objects"), list)
            else 0,
            "runtime_symbol_count": len(runtime_symbols),
            "undefined_symbol_count": len(undefined_symbols),
            "defined_symbol_count": len(defined_symbols),
            "required_c_api_symbol_count": len(required_c_api_symbols),
            "required_capsule_count": len(required_capsules),
            "project_generated_c_api_symbol_count": len(
                project_generated_c_api_symbols
            ),
            "project_generated_c_api_prefix_count": len(
                project_generated_c_api_prefixes
            ),
            "root_symbol": object_closure.get("root_symbol")
            if isinstance(object_closure.get("root_symbol"), str)
            else None,
            "init_symbol_owner": object_closure.get("init_symbol_owner")
            if isinstance(object_closure.get("init_symbol_owner"), str)
            else None,
            "keys": sorted(str(key) for key in object_closure.keys()),
        }
        if require_object_closure and not (
            object_closure_summary["has_closure_sha256"]
            or object_closure_summary["object_count"]
            or object_closure_summary["runtime_symbol_count"]
            or object_closure_summary["undefined_symbol_count"]
            or object_closure_summary["defined_symbol_count"]
            or object_closure_summary["required_c_api_symbol_count"]
            or object_closure_summary["required_capsule_count"]
            or object_closure_summary["project_generated_c_api_symbol_count"]
            or object_closure_summary["project_generated_c_api_prefix_count"]
        ):
            errors.append("object_closure is empty")
    elif require_object_closure:
        errors.append("object_closure missing")

    package_errors: list[str] = []
    module_name = manifest.get("module")
    package = ""
    if isinstance(module_name, str) and module_name.strip():
        package = module_name.strip().split(".", 1)[0]
    elif require_python_export or require_callable_export:
        errors.append(
            "Cannot enforce required public exports because extension manifest "
            "has no valid 'module' package root."
        )

    python_exports: tuple[str, ...] = ()
    callable_export_names: tuple[str, ...] = ()
    required_python_exports: tuple[str, ...] = ()
    required_callable_exports: tuple[str, ...] = ()
    if package:
        python_exports = _manifest_dotted_name_tuple(
            manifest,
            "python_exports",
            package=package,
            errors=package_errors,
        )
        callable_export_names = tuple(
            export.qualified_name
            for export in _manifest_callable_exports(
                manifest,
                package=package,
                errors=package_errors,
            )
        )
        if require_python_export:
            required_python_exports = _manifest_dotted_name_tuple(
                {"required_python_exports": require_python_export},
                "required_python_exports",
                package=package,
                errors=package_errors,
            )
        if require_callable_export:
            required_callable_exports = _manifest_dotted_name_tuple(
                {"required_callable_exports": require_callable_export},
                "required_callable_exports",
                package=package,
                errors=package_errors,
            )
    errors.extend(package_errors)

    missing_python_exports = sorted(set(required_python_exports) - set(python_exports))
    missing_callable_exports = sorted(
        set(required_callable_exports) - set(callable_export_names)
    )
    for name in missing_python_exports:
        errors.append(
            f"Missing required python export {name!r}; rebuild or republish the "
            "source-recompiled extension artifact with "
            f"`molt extension build --python-export {name}` so package admission "
            "is manifest-symbol driven."
        )
    for name in missing_callable_exports:
        errors.append(
            f"Missing required callable export {name!r}; rebuild or republish the "
            "source-recompiled extension artifact with matching "
            "`--callable-export-json` metadata so native calls do not fall back "
            "to CALL_BIND or fake module__function symbols."
        )

    status = "ok" if not errors else "error"
    if json_output:
        payload = _json_payload(
            "extension-audit",
            status,
            data={
                "path": str(target),
                "manifest_source": manifest_source,
                "wheel": str(wheel_path) if wheel_path is not None else None,
                "molt_c_api_version": manifest_abi,
                "abi_tag": manifest_abi_tag,
                "capabilities": manifest_capabilities,
                "loader_kind": loader_kind,
                "runtime_linkage": runtime_linkage,
                "artifact_kind": artifact_kind,
                "extension": extension_entry,
                "extension_path": str(extension_path)
                if extension_path is not None
                else None,
                "extension_file_status": extension_file_status,
                "object_closure": object_closure_summary,
                "require_capabilities": require_capabilities,
                "require_abi": required_abi,
                "require_checksum": require_checksum,
                "require_loader_kind": required_loader_kind,
                "require_runtime_linkage": required_runtime_linkage,
                "require_artifact_kind": required_artifact_kind,
                "require_artifact_file": require_artifact_file,
                "require_object_closure": require_object_closure,
                "python_exports": list(python_exports),
                "callable_exports": list(callable_export_names),
                "required_python_exports": list(required_python_exports),
                "required_callable_exports": list(required_callable_exports),
                "missing_python_exports": missing_python_exports,
                "missing_callable_exports": missing_callable_exports,
                "wheel_tags": {
                    "python": wheel_tags[0],
                    "abi": wheel_tags[1],
                    "platform": wheel_tags[2],
                }
                if wheel_tags is not None
                else None,
            },
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        if errors:
            for err in errors:
                print(f"ERROR: {err}")
        else:
            print(f"Extension audit passed: {target}")
        if verbose:
            print(f"Manifest source: {manifest_source}")
            if wheel_path is not None:
                print(f"Wheel: {wheel_path}")
            for warning in warnings:
                print(f"WARN: {warning}")
    return 0 if not errors else 1
