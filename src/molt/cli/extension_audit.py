from __future__ import annotations

import json
from pathlib import Path
from typing import Any
import zipfile

from molt.cli.extension_manifest import (
    _MOLT_C_API_VERSION_RE,
    _validate_extension_manifest,
)
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload


def extension_audit(
    path: str,
    require_capabilities: bool = False,
    require_abi: str | None = None,
    require_checksum: bool = False,
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
        require_checksum=require_checksum,
        warn_missing_checksum=not require_checksum,
    )
    errors.extend(validation.errors)
    warnings.extend(validation.warnings)
    wheel_path = validation.wheel_path
    manifest_abi = validation.abi_version
    manifest_abi_tag = validation.abi_tag
    manifest_capabilities = validation.capabilities
    wheel_tags = validation.wheel_tags

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
                "require_capabilities": require_capabilities,
                "require_abi": required_abi,
                "require_checksum": require_checksum,
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
